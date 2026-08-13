//! Windows callback backing 与 FFI trampoline。

use std::cell::Cell;
use std::os::raw::{c_uint, c_void};
use std::slice;
use std::sync::{Arc, Mutex, MutexGuard};

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

/// FFI callback 内只能吞掉 panic；forget payload 可避免其 Drop 再次 panic。
fn catch_and_forget_panic(function: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(function)) {
        Ok(()) => false,
        Err(payload) => {
            std::mem::forget(payload);
            true
        }
    }
}

fn drop_without_unwind<T>(value: T) {
    catch_and_forget_panic(|| drop(value));
}

/// SDK 仅保存 `pUser`，Arc 的 native strong ref 使地址稳定到 DestroyHandle。
pub(super) struct CallbackSlot<C> {
    callback: Mutex<Option<Arc<C>>>,
}

impl<C> CallbackSlot<C> {
    pub(super) fn new() -> Self {
        Self {
            callback: Mutex::new(None),
        }
    }

    /// 安装一次 closure；旧的 in-flight closure 由其临时 Arc 保活。
    pub(super) fn set(&self, callback: C) {
        let previous = self.lock().replace(Arc::new(callback));
        drop_without_unwind(previous);
    }

    /// 静默后续 callback；已经进入 trampoline 的 closure 可以自然返回。
    pub(super) fn clear(&self) {
        let callback = self.lock().take();
        drop_without_unwind(callback);
    }

    fn load(&self) -> Option<Arc<C>> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<C>>> {
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

unsafe fn invoke_slot<C>(user: *mut c_void, invoke: impl FnOnce(&C)) {
    if user.is_null() {
        return;
    }

    // slot 操作和用户 closure 都留在最外层 boundary 内，禁止 Rust panic 穿过 FFI。
    catch_and_forget_panic(|| {
        let _depth = CallbackDepthGuard::enter();
        let ptr = user.cast::<CallbackSlot<C>>();
        // SAFETY: 注册时为 SDK 单独保留一个 raw Arc strong ref。
        unsafe { Arc::increment_strong_count(ptr) };
        // SAFETY: 上一步为本次调用增加了一个 strong ref。
        let slot = unsafe { Arc::from_raw(ptr) };
        let callback = slot.load();

        if let Some(callback) = callback {
            if catch_and_forget_panic(|| invoke(callback.as_ref())) {
                // panic 后静默该 slot，避免 SDK thread 重复进入同一异常 closure。
                slot.clear();
            }
            drop_without_unwind(callback);
        }
        drop_without_unwind(slot);
    });
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::camera::{EventCallback, ExceptionCallback};
    use crate::sys;

    use super::{CallbackSlot, event_trampoline, exception_trampoline};

    // 核心 FFI 约定：用户 panic 不得越过 extern "C" 边界。
    #[test]
    fn callback_panic_is_contained_at_ffi_boundary() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let slot = Arc::new(CallbackSlot::<ExceptionCallback>::new());
        slot.set(Box::new(move |_| {
            callback_calls.fetch_add(1, Ordering::Relaxed);
            panic!("callback panic");
        }));
        let native = Arc::into_raw(Arc::clone(&slot));

        // SAFETY: native 是本测试为 trampoline 保留的匹配 Arc strong ref。
        unsafe { exception_trampoline(1, native.cast_mut().cast::<c_void>()) };
        // 首次 panic 后 slot 已静默，后续 native callback 不再调用用户 closure。
        unsafe { exception_trampoline(1, native.cast_mut().cast::<c_void>()) };

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // SAFETY: 测试结束且不会再次调用 trampoline，回收 native strong ref。
        unsafe { drop(Arc::from_raw(native)) };
    }

    // 核心转换：event name 有界读取，timestamp 合并高低位。
    #[test]
    fn event_trampoline_converts_name_and_timestamp() {
        let observed = Arc::new(Mutex::new(None::<(String, u64)>));
        let callback_observed = Arc::clone(&observed);
        let slot = Arc::new(CallbackSlot::<EventCallback>::new());
        slot.set(Box::new(move |event| {
            *callback_observed.lock().unwrap() =
                Some((event.name().into_owned(), event.timestamp()));
        }));
        let native = Arc::into_raw(Arc::clone(&slot));
        let mut raw = sys::MV_EVENT_OUT_INFO {
            nTimestampHigh: 0x1020_3040,
            nTimestampLow: 0x5060_7080,
            ..Default::default()
        };
        for (target, source) in raw.EventName.iter_mut().zip(b"ExposureEnd\0") {
            *target = *source as _;
        }

        // SAFETY: raw 与匹配的 native Arc 在同步调用期间有效。
        unsafe { event_trampoline(&mut raw, native.cast_mut().cast::<c_void>()) };

        assert_eq!(
            *observed.lock().unwrap(),
            Some(("ExposureEnd".into(), 0x1020_3040_5060_7080))
        );
        // SAFETY: 测试结束且不会再次调用 trampoline。
        unsafe { drop(Arc::from_raw(native)) };
    }
}
