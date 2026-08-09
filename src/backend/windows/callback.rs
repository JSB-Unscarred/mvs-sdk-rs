use std::cell::RefCell;
use std::os::raw::{c_uchar, c_uint, c_void};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::callback::EventInfo;
use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::frame::Frame;
use crate::sys;

use super::frame::{data_len_from_raw, info_from_raw};

thread_local! {
    static ACTIVE_CALLBACK_SLOTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// 传给 SDK 的稳定 callback backing。
///
/// `Camera` 以 `Box` 持有该值，直到 handle 销毁成功；若销毁结果不确定，调用方需保留
/// backing，防止 SDK 再次使用 `pUser`。`deactivate` 会等待 in-flight callback，因此不能
/// 从当前 slot 的 callback 中调用。
pub(super) struct CallbackSlot<C> {
    accepting: AtomicBool,
    callback: Mutex<Option<C>>,
}

impl<C> CallbackSlot<C> {
    pub(super) fn new() -> Self {
        Self {
            accepting: AtomicBool::new(false),
            callback: Mutex::new(None),
        }
    }

    /// 当前线程是否正在执行该 slot，用于避免 callback 内自等待。
    pub(super) fn is_current(&self) -> bool {
        let address = self.address();
        ACTIVE_CALLBACK_SLOTS
            .try_with(|slots| {
                slots
                    .try_borrow()
                    .is_ok_and(|slots| slots.contains(&address))
            })
            .unwrap_or(false)
    }

    pub(super) fn is_active(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
    }

    /// 安装 closure 并开始接收 callback；旧 closure 交给调用方在锁外释放。
    pub(super) fn activate(&self, callback: C) -> Option<C> {
        let mut current = self.lock_callback();
        let previous = current.replace(callback);
        self.accepting.store(true, Ordering::Release);
        previous
    }

    pub(super) fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    /// 停止接收并等待 in-flight callback 退出。
    pub(super) fn deactivate(&self) -> Option<C> {
        self.stop_accepting();
        self.lock_callback().take()
    }

    fn address(&self) -> usize {
        self as *const Self as usize
    }

    fn lock_callback(&self) -> MutexGuard<'_, Option<C>> {
        self.callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct CallbackContextGuard {
    address: usize,
}

impl CallbackContextGuard {
    fn enter(address: usize) -> Option<Self> {
        let entered = ACTIVE_CALLBACK_SLOTS
            .try_with(|slots| {
                let Ok(mut slots) = slots.try_borrow_mut() else {
                    return false;
                };
                if slots.contains(&address) {
                    return false;
                }
                slots.push(address);
                true
            })
            .unwrap_or(false);

        entered.then_some(Self { address })
    }
}

impl Drop for CallbackContextGuard {
    fn drop(&mut self) {
        let _ = ACTIVE_CALLBACK_SLOTS.try_with(|slots| {
            let Ok(mut slots) = slots.try_borrow_mut() else {
                return;
            };
            if let Some(index) = slots.iter().rposition(|&slot| slot == self.address) {
                slots.remove(index);
            }
        });
    }
}

pub(super) unsafe extern "C" fn image_trampoline(
    data: *mut c_uchar,
    info: *mut sys::MV_FRAME_OUT_INFO_EX,
    user: *mut c_void,
) {
    // SAFETY: Camera 在 native handle 可能调用 callback 期间保留 slot backing。
    unsafe {
        invoke_slot::<ImageCallbackFn>(user, |function| {
            if info.is_null() {
                return;
            }

            let raw_info = &*info;
            let info = info_from_raw(raw_info);
            let Some(len) = data_len_from_raw(data, raw_info) else {
                return;
            };
            let bytes = if len == 0 {
                &[]
            } else {
                // SAFETY: SDK 在 callback 返回前持有图像，且长度已通过 slice 前置校验。
                slice::from_raw_parts(data, len)
            };
            (function)(&Frame::from_parts(bytes, info));
        });
    }
}

pub(super) unsafe extern "C" fn exception_trampoline(msg_type: c_uint, user: *mut c_void) {
    // SAFETY: Camera 在 native handle 可能调用 callback 期间保留 slot backing。
    unsafe {
        invoke_slot::<ExceptionCallbackFn>(user, |function| function(msg_type));
    }
}

pub(super) unsafe extern "C" fn event_trampoline(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) {
    // SAFETY: Camera 在 native handle 可能调用 callback 期间保留 slot backing。
    unsafe {
        invoke_slot::<EventCallbackFn>(user, |function| {
            if info.is_null() {
                return;
            }

            let raw = &*info;
            let name_len = raw
                .EventName
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(raw.EventName.len());
            let name = slice::from_raw_parts(raw.EventName.as_ptr().cast::<u8>(), name_len);
            let event = EventInfo::new(
                name,
                raw.nEventID,
                raw.nStreamChannel,
                ((raw.nBlockIdHigh as u64) << 32) | raw.nBlockIdLow as u64,
                ((raw.nTimestampHigh as u64) << 32) | raw.nTimestampLow as u64,
            );
            (function)(&event);
        });
    }
}

unsafe fn invoke_slot<C>(user: *mut c_void, invoke: impl FnOnce(&mut C)) {
    if user.is_null() {
        return;
    }
    let _activity = crate::library::enter_callback();

    // SAFETY: pUser 指向 Camera 持有或在销毁失败后保留的 Box<CallbackSlot<C>>。
    let slot = unsafe { &*user.cast::<CallbackSlot<C>>() };
    let Some(_context) = CallbackContextGuard::enter(slot.address()) else {
        // FnMut 不允许同一 slot 同步重入。
        return;
    };

    if !slot.accepting.load(Ordering::Acquire) {
        return;
    }

    let mut callback = slot.lock_callback();
    if !slot.accepting.load(Ordering::Acquire) {
        let removed = callback.take();
        drop(callback);
        if let Some(callback) = removed {
            drop_callback_safely(callback);
        }
        return;
    }
    let Some(function) = callback.as_mut() else {
        return;
    };

    // callback panic 必须在 extern C 边界内终止传播；panic 后停用该 slot。
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoke(function))).is_err() {
        slot.stop_accepting();
    }

    let removed = if slot.accepting.load(Ordering::Acquire) {
        None
    } else {
        callback.take()
    };
    drop(callback);
    if let Some(callback) = removed {
        drop_callback_safely(callback);
    }
}

/// 在可能位于 FFI callback 的路径中释放用户 closure，阻止 Drop panic 外泄。
pub(super) fn drop_callback_safely<C>(callback: C) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(callback)));
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use crate::camera::{
        EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    };
    use crate::sys;

    use super::{CallbackSlot, drop_callback_safely, event_trampoline, exception_trampoline};

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    // 验证 closure panic 被 FFI 边界截获，并停用 slot 以屏蔽后续调用。
    #[test]
    fn callback_panic_disables_the_slot() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let slot = Box::new(CallbackSlot::<ExceptionCallbackFn>::new());
        assert!(
            slot.activate(Box::new(move |_| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
                panic!("callback panic");
            }))
            .is_none()
        );
        let user = std::ptr::from_ref(slot.as_ref())
            .cast_mut()
            .cast::<c_void>();

        // SAFETY: user 指向本测试持有的匹配 slot，且两次调用期间地址稳定。
        unsafe { exception_trampoline(1, user) };
        unsafe { exception_trampoline(2, user) };

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!slot.is_active());
        assert!(slot.deactivate().is_none());
    }

    // 验证 deactivate 先停止接收，并等待当前 FnMut 调用退出后再释放 closure。
    #[test]
    fn deactivate_waits_for_in_flight_callback_before_drop() {
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&drops));
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let slot = Box::new(CallbackSlot::<ExceptionCallbackFn>::new());
        assert!(
            slot.activate(Box::new(move |_| {
                let _ = &probe;
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }))
            .is_none()
        );
        let user = std::ptr::from_ref(slot.as_ref()) as usize;
        let (deactivated_tx, deactivated_rx) = mpsc::sync_channel(1);

        std::thread::scope(|scope| {
            let callback = scope.spawn(move || {
                // SAFETY: scoped thread 返回前 slot 地址稳定且类型匹配。
                unsafe { exception_trampoline(1, user as *mut c_void) };
            });
            entered_rx.recv().unwrap();

            let slot_ref = slot.as_ref();
            let deactivate = scope.spawn(move || {
                if let Some(callback) = slot_ref.deactivate() {
                    drop_callback_safely(callback);
                }
                deactivated_tx.send(()).unwrap();
            });
            while slot.is_active() {
                std::thread::yield_now();
            }

            assert_eq!(drops.load(Ordering::Relaxed), 0);
            assert!(matches!(
                deactivated_rx.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release_tx.send(()).unwrap();
            callback.join().unwrap();
            deactivated_rx.recv().unwrap();
            deactivate.join().unwrap();
        });

        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    // 验证 event trampoline 复制事件名与高低位 metadata。
    #[test]
    fn event_trampoline_converts_borrowed_metadata() {
        type Observed = (String, u16, u16, u64, u64);

        let observed = Arc::new(Mutex::new(None::<Observed>));
        let callback_observed = Arc::clone(&observed);
        let slot = Box::new(CallbackSlot::<EventCallbackFn>::new());
        assert!(
            slot.activate(Box::new(move |event| {
                *callback_observed.lock().unwrap() = Some((
                    event.name().into_owned(),
                    event.event_id(),
                    event.stream_channel(),
                    event.block_id(),
                    event.timestamp(),
                ));
            }))
            .is_none()
        );

        let mut raw = sys::MV_EVENT_OUT_INFO {
            nEventID: 7,
            nStreamChannel: 3,
            nBlockIdHigh: 0x0123_4567,
            nBlockIdLow: 0x89AB_CDEF,
            nTimestampHigh: 0x1020_3040,
            nTimestampLow: 0x5060_7080,
            ..Default::default()
        };
        for (target, source) in raw.EventName.iter_mut().zip(b"ExposureEnd\0") {
            *target = *source as _;
        }
        let user = std::ptr::from_ref(slot.as_ref())
            .cast_mut()
            .cast::<c_void>();

        // SAFETY: raw 与匹配的 slot 在同步 trampoline 调用期间有效。
        unsafe { event_trampoline(&mut raw, user) };

        assert_eq!(
            *observed.lock().unwrap(),
            Some((
                "ExposureEnd".into(),
                7,
                3,
                0x0123_4567_89AB_CDEF,
                0x1020_3040_5060_7080,
            ))
        );
        drop_callback_safely(slot.deactivate().expect("active event callback"));
    }
}
