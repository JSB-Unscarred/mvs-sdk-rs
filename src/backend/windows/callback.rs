use std::os::raw::{c_uchar, c_uint, c_void};
use std::slice;
use std::sync::{Arc, Mutex};

use crate::callback::EventInfo;
use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::frame::Frame;
use crate::sys;

use super::frame::metadata_from_raw;

pub(super) struct ImageCallback(pub Mutex<ImageCallbackFn>);
pub(super) struct ExceptionCallback(pub Mutex<ExceptionCallbackFn>);
pub(super) struct EventCallback(pub Mutex<EventCallbackFn>);

/// One strong `Arc` reference transferred into a stable raw token for the
/// native callback API. The token is reclaimed on drop after native teardown.
pub(super) struct CallbackRegistration<T> {
    raw: *const T,
}

impl<T> CallbackRegistration<T> {
    pub(super) fn new(callback: T) -> Self {
        Self {
            raw: Arc::into_raw(Arc::new(callback)),
        }
    }

    pub(super) fn user_data(&self) -> *mut c_void {
        self.raw.cast_mut().cast()
    }
}

// SAFETY: CallbackRegistration owns exactly the strong Arc token represented
// by `raw`, so moving it has the same requirements as moving an Arc<T>.
unsafe impl<T: Send + Sync> Send for CallbackRegistration<T> {}

impl<T> Drop for CallbackRegistration<T> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from Arc::into_raw in `new` and this Drop consumes
        // that exact strong reference once.
        unsafe {
            drop(Arc::from_raw(self.raw));
        }
    }
}

pub(super) unsafe extern "C" fn image_trampoline(
    data: *mut c_uchar,
    info: *mut sys::MV_FRAME_OUT_INFO_EX,
    user: *mut c_void,
) {
    if user.is_null() || info.is_null() {
        return;
    }

    // SAFETY: all pointers are supplied by the SDK for this callback, and
    // `user` is an Arc::into_raw token retained by the camera backend.
    unsafe {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let callback = clone_callback::<ImageCallback>(user);
            let raw_info = &*info;
            let metadata = metadata_from_raw(raw_info);
            let len = raw_info.nFrameLen as usize;
            let bytes = if data.is_null() || len == 0 {
                &[]
            } else {
                slice::from_raw_parts(data, len)
            };
            let frame = Frame::from_parts(bytes, &metadata);
            let mut function = callback
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (function)(&frame);
        }));

        if let Err(panic_info) = result {
            log_callback_panic("Image", panic_info);
        }
    }
}

pub(super) unsafe extern "C" fn exception_trampoline(msg_type: c_uint, user: *mut c_void) {
    if user.is_null() {
        return;
    }

    // SAFETY: `user` is an Arc::into_raw token retained by the camera.
    unsafe {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let callback = clone_callback::<ExceptionCallback>(user);
            let mut function = callback
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (function)(msg_type);
        }));

        if let Err(panic_info) = result {
            log_callback_panic("Exception", panic_info);
        }
    }
}

pub(super) unsafe extern "C" fn event_trampoline(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) {
    if user.is_null() || info.is_null() {
        return;
    }

    // SAFETY: pointers are valid for this callback invocation.
    unsafe {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let callback = clone_callback::<EventCallback>(user);
            let raw = &*info;
            let name_len = raw
                .EventName
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(raw.EventName.len());
            let name = slice::from_raw_parts(raw.EventName.as_ptr() as *const u8, name_len);
            let event = EventInfo::new(
                name,
                raw.nEventID,
                raw.nStreamChannel,
                ((raw.nBlockIdHigh as u64) << 32) | raw.nBlockIdLow as u64,
                ((raw.nTimestampHigh as u64) << 32) | raw.nTimestampLow as u64,
            );
            let mut function = callback
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (function)(&event);
        }));

        if let Err(panic_info) = result {
            log_callback_panic("Event", panic_info);
        }
    }
}

unsafe fn clone_callback<T>(user: *mut c_void) -> Arc<T> {
    let pointer = user.cast::<T>().cast_const();
    // SAFETY: the pointer came from Arc::into_raw, and the camera backend
    // retains that original strong token until after native handle teardown.
    unsafe {
        Arc::increment_strong_count(pointer);
        Arc::from_raw(pointer)
    }
}

fn log_callback_panic(callback_name: &str, panic_info: Box<dyn std::any::Any + Send>) {
    // Diagnostics must not be able to unwind across the extern callback
    // boundary either. Ignore stderr failures and catch any unexpected
    // formatting panic as a final guard.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use std::io::Write as _;

        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(
            stderr,
            "[mvs_sdk_rs] {callback_name} callback panicked; panic was caught before crossing the FFI boundary."
        );
        if let Some(message) = panic_info.downcast_ref::<&str>() {
            let _ = writeln!(stderr, "  Panic message: {message}");
        } else if let Some(message) = panic_info.downcast_ref::<String>() {
            let _ = writeln!(stderr, "  Panic message: {message}");
        } else {
            let _ = writeln!(stderr, "  Panic message: <unavailable>");
        }
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{CallbackRegistration, clone_callback};

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn registration_and_trampoline_clone_balance_arc_tokens() {
        let drops = Arc::new(AtomicUsize::new(0));
        let registration = CallbackRegistration::new(DropProbe(Arc::clone(&drops)));

        // SAFETY: user_data returns the live Arc::into_raw token owned by
        // `registration`.
        let callback = unsafe { clone_callback::<DropProbe>(registration.user_data()) };
        drop(registration);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        drop(callback);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
