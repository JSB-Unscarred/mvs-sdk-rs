//! Windows callback backing 与 FFI trampoline。

use std::cell::Cell;
use std::os::raw::{c_uint, c_void};
use std::slice;
use std::sync::{Mutex, MutexGuard};

use crate::camera::{EventCallback, ExceptionCallback, ImageCallback};
use crate::frame::Frame;
use crate::sys;

use super::frame::{data_len_from_raw, info_from_raw};
use crate::callback::EventInfo;

thread_local! {
    /// 只区分“当前是否处于 MVS callback”，用于拒绝 native 生命周期重入。
    static CALLBACK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

pub(super) fn in_callback() -> bool {
    CALLBACK_DEPTH.with(|depth| depth.get() != 0)
}

struct CallbackDepthGuard;

impl CallbackDepthGuard {
    fn enter() -> Self {
        CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for CallbackDepthGuard {
    fn drop(&mut self) {
        CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Camera-owned Box 提供稳定的 `pUser` 地址，closure Arc 保护在途调用。
pub(super) struct CallbackSlot<C> {
    callback: Mutex<Option<C>>,
}

impl<C: Clone> CallbackSlot<C> {
    pub(super) fn new() -> Self {
        Self {
            callback: Mutex::new(None),
        }
    }

    /// 安装一次 closure；旧的 in-flight closure 由其临时 Arc 保活。
    pub(super) fn set(&self, callback: C) {
        let previous = self.lock().replace(callback);
        drop(previous);
    }

    /// 静默后续 callback；已经进入 trampoline 的 closure 可以自然返回。
    pub(super) fn clear(&self) {
        let callback = self.lock().take();
        drop(callback);
    }

    fn load(&self) -> Option<C> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Option<C>> {
        self.callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(super) unsafe extern "C" fn image_trampoline(
    frame: *mut sys::MV_FRAME_OUT,
    user: *mut c_void,
    _auto_free: sys::bool_,
) {
    // SAFETY: pUser 对应注册时为 native 保留的 ImageCallback slot。
    unsafe {
        invoke_slot::<ImageCallback>(user, |function| {
            let Some(raw) = frame.as_ref() else {
                return;
            };
            let info = info_from_raw(&raw.stFrameInfo);
            let len = data_len_from_raw(&raw.stFrameInfo);
            let data = if len == 0 {
                &[]
            } else {
                // SAFETY: Ex2(autoFree=true) 保证 buffer 在 callback 返回前有效。
                slice::from_raw_parts(raw.pBufAddr, len)
            };
            function(&Frame::from_parts(data, info));
        });
    }
}

pub(super) unsafe extern "C" fn exception_trampoline(msg_type: c_uint, user: *mut c_void) {
    // SAFETY: pUser 对应注册时为 native 保留的 ExceptionCallback slot。
    unsafe { invoke_slot::<ExceptionCallback>(user, |function| function(msg_type)) };
}

pub(super) unsafe extern "C" fn event_trampoline(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) {
    // SAFETY: pUser 对应注册时为 native 保留的 EventCallback slot。
    unsafe {
        invoke_slot::<EventCallback>(user, |function| {
            let Some(raw) = info.as_ref() else {
                return;
            };
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
            function(&event);
        });
    }
}

unsafe fn invoke_slot<C: Clone>(user: *mut c_void, invoke: impl FnOnce(&C)) {
    if user.is_null() {
        return;
    }

    let _depth = CallbackDepthGuard::enter();
    // SAFETY: pUser 指向 Camera 持有或 DestroyHandle 失败后泄漏的 Box slot。
    let slot = unsafe { &*user.cast::<CallbackSlot<C>>() };
    let Some(callback) = slot.load() else {
        return;
    };

    // panic 不得穿过 extern "C"；库在 ABI 边界终止进程。
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoke(&callback))).is_err() {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::camera::{EventCallback, ExceptionCallback};
    use crate::sys;

    use super::{CallbackSlot, event_trampoline};

    // 注销清空 slot 后，已取得的 closure Arc 仍能完成在途调用。
    #[test]
    fn loaded_callback_outlives_slot_clear() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let slot = CallbackSlot::<ExceptionCallback>::new();
        slot.set(Arc::new(move |_| {
            callback_calls.fetch_add(1, Ordering::Relaxed);
        }));

        let callback = slot.load().expect("callback installed");
        slot.clear();
        callback(1);

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(slot.load().is_none());
    }

    // 核心转换：event name 有界读取，timestamp 合并高低位。
    #[test]
    fn event_trampoline_converts_name_and_timestamp() {
        let observed = Arc::new(Mutex::new(None::<(Vec<u8>, u64)>));
        let callback_observed = Arc::clone(&observed);
        let slot = Box::new(CallbackSlot::<EventCallback>::new());
        slot.set(Arc::new(move |event| {
            *callback_observed.lock().unwrap() = Some((event.name().to_vec(), event.timestamp()));
        }));
        let user = std::ptr::from_ref(slot.as_ref())
            .cast_mut()
            .cast::<c_void>();
        let mut raw = sys::MV_EVENT_OUT_INFO {
            nTimestampHigh: 0x1020_3040,
            nTimestampLow: 0x5060_7080,
            ..Default::default()
        };
        for (target, source) in raw.EventName.iter_mut().zip(b"ExposureEnd\0") {
            *target = *source as _;
        }

        // SAFETY: raw 与 Box slot 在同步调用期间有效。
        unsafe { event_trampoline(&mut raw, user) };

        assert_eq!(
            *observed.lock().unwrap(),
            Some((b"ExposureEnd".to_vec(), 0x1020_3040_5060_7080))
        );
    }
}
