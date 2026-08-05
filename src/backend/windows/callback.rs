use std::cell::RefCell;
use std::os::raw::{c_uchar, c_uint, c_void};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

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

/// A stable, per-camera callback location passed to the native SDK as
/// `pUser`.
///
/// # Native lifetime contract
///
/// The owning camera keeps an [`Arc`] reference to this slot until
/// `MV_CC_DestroyHandle(handle)` returns `MV_OK`. Every trampoline acquires a
/// temporary strong reference on entry, so a supported teardown from an
/// exception callback may release the camera's reference while the current
/// invocation keeps the slot alive until it returns. A successful handle
/// destruction is treated as the quiescence boundary: except for that current
/// exception invocation, the SDK must not subsequently start, resume, or
/// retain a callback that can access this slot, including one already
/// dispatched but not yet executing its first Rust instruction (or an event
/// name passed alongside it). Register, unregister, stop, and close are not
/// otherwise assumed to drain callbacks, so admission can be revoked
/// independently while the allocation stays put.
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

    /// Install the current closure and admit callbacks. The old closure, if
    /// any, is returned so its user-controlled destructor can run outside the
    /// slot mutex and inside an unwind boundary.
    pub(super) fn activate(&self, callback: C) -> Option<C> {
        let mut current = self.lock_callback();
        let previous = current.replace(callback);
        self.accepting.store(true, Ordering::Release);
        previous
    }

    pub(super) fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    pub(super) fn take_callback(&self) -> Option<C> {
        self.lock_callback().take()
    }

    pub(super) fn deactivate(&self) -> Option<C> {
        self.stop_accepting();
        self.take_callback()
    }

    pub(super) fn deactivate_nonblocking(&self) -> Option<C> {
        self.stop_accepting();
        match self.callback.try_lock() {
            Ok(mut callback) => callback.take(),
            Err(TryLockError::Poisoned(error)) => error.into_inner().take(),
            Err(TryLockError::WouldBlock) => None,
        }
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
    // SAFETY: the camera retains the slot backing through native teardown.
    unsafe {
        invoke_slot::<ImageCallbackFn>(user, "Image", |function| {
            if info.is_null() {
                return;
            }

            // Native frame pointers are intentionally not read until the
            // slot has admitted this invocation under its mutex.
            let raw_info = &*info;
            let info = info_from_raw(raw_info);
            let Some(len) = data_len_from_raw(data, raw_info) else {
                return;
            };
            let bytes = if len == 0 {
                &[]
            } else {
                // SAFETY: the SDK owns the image buffer for this callback.
                // The shared validator rejected null, oversized, and wrapping
                // ranges before constructing this borrowed slice.
                slice::from_raw_parts(data, len)
            };
            let frame = Frame::from_parts(bytes, info);
            (function)(&frame);
        });
    }
}

pub(super) unsafe extern "C" fn exception_trampoline(msg_type: c_uint, user: *mut c_void) {
    // SAFETY: the camera retains the slot backing through native teardown.
    unsafe {
        invoke_slot::<ExceptionCallbackFn>(user, "Exception", |function| {
            (function)(msg_type);
        });
    }
}

pub(super) unsafe extern "C" fn event_trampoline(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) {
    // SAFETY: the camera retains the slot backing through native teardown.
    unsafe {
        invoke_slot::<EventCallbackFn>(user, "Event", |function| {
            if info.is_null() {
                return;
            }

            // Native event data is borrowed only while the admitted closure
            // holds the slot mutex.
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

unsafe fn invoke_slot<C>(user: *mut c_void, callback_name: &str, invoke: impl FnOnce(&mut C)) {
    // Count from the first Rust instruction through every return path. This
    // guard is deliberately independent of the lifecycle read lock: a
    // callback may ask to shut the SDK down, which must fail as in-use rather
    // than deadlocking on a lock held by the same callback.
    let _activity = crate::library::enter_callback();
    if user.is_null() {
        return;
    }

    let slot_ptr = user.cast::<CallbackSlot<C>>();
    // SAFETY: `user` was originally produced by `Arc::into_raw` for the live
    // CallbackSlot<C> registered for this exact trampoline type. The camera
    // retains the reconstructed associated strong reference until successful
    // handle destruction (or leaks it when destruction is uncertain), so the
    // count is non-zero whenever the SDK is permitted to execute this
    // instruction. This relies on successful destruction quiescing even an
    // invocation already dispatched but not yet here. An exception callback
    // that destroys its camera already owns the temporary reference acquired
    // here.
    unsafe { Arc::increment_strong_count(slot_ptr) };
    // SAFETY: the increment above created the strong reference reconstructed
    // here. It is released on every return path when `slot` is dropped.
    let slot = unsafe { Arc::from_raw(slot_ptr) };
    let Some(_context) = CallbackContextGuard::enter(slot.address()) else {
        // FnMut is serialized by a non-reentrant mutex. Reject synchronous
        // recursion into the same slot instead of self-deadlocking.
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

    // Keep the mutex guard outside catch_unwind. A user panic is caught while
    // the guard remains live, so the mutex is not poisoned. The TLS guard and
    // mutex also remain active while the panic payload is diagnosed/dropped;
    // that payload may itself own and drop this Camera.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoke(function)));
    if let Err(panic_info) = result {
        slot.stop_accepting();
        log_callback_panic(callback_name, panic_info);
    }

    // A callback may consume/drop its own Camera. That path revokes admission
    // but cannot lock this current slot. Finish removing the closure with the
    // guard already held, then destroy its captures outside the mutex while
    // the callback-context guard is still present.
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

pub(super) fn drop_callback_safely<C>(callback: C) {
    catch_and_forget_panic(|| drop(callback));
}

fn log_callback_panic(callback_name: &str, panic_info: Box<dyn std::any::Any + Send>) {
    // Diagnostics must not be able to unwind across the extern callback
    // boundary either. Ignore stderr failures and catch any unexpected
    // formatting panic as a final guard.
    catch_and_forget_panic(|| {
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
    });

    // A panic payload is user-controlled and may itself panic in Drop. Reuse
    // the same containment used for closure captures.
    drop_callback_safely(panic_info);
}

fn catch_and_forget_panic(f: impl FnOnce()) {
    if let Err(panic_info) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        // Dropping a caught payload can execute another user-defined Drop and
        // panic again. Forgetting this final payload closes that recursion.
        std::mem::forget(panic_info);
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::camera::{
        ExceptionCallback as ExceptionCallbackFn, ImageCallback as ImageCallbackFn,
    };
    use crate::{PixelType, sys};

    use super::{
        CallbackSlot, drop_callback_safely, exception_trampoline, image_trampoline,
        log_callback_panic,
    };

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("panic while dropping a callback-owned value");
        }
    }

    struct NativeSlot<C> {
        slot: Arc<CallbackSlot<C>>,
        user: *const CallbackSlot<C>,
    }

    impl<C> NativeSlot<C> {
        fn new() -> Self {
            let user = Arc::into_raw(Arc::new(CallbackSlot::new()));
            // SAFETY: `user` was just produced by Arc::into_raw and is
            // reconstructed exactly once into this test owner's Arc.
            let slot = unsafe { Arc::from_raw(user) };
            Self { slot, user }
        }

        fn user_data(&self) -> *mut c_void {
            self.user.cast_mut().cast()
        }
    }

    impl<C> std::ops::Deref for NativeSlot<C> {
        type Target = CallbackSlot<C>;

        fn deref(&self) -> &Self::Target {
            &self.slot
        }
    }

    // 验证 image trampoline 按 extended frame 字段构造 callback view。
    #[test]
    fn image_trampoline_uses_extended_frame_shape_and_length() {
        let observed = Arc::new(Mutex::new(None));
        let callback_observed = Arc::clone(&observed);
        let slot = NativeSlot::<ImageCallbackFn>::new();
        assert!(
            slot.activate(Box::new(move |frame| {
                let info = frame.info();
                *callback_observed.lock().unwrap() = Some((
                    frame.data().len(),
                    info.width(),
                    info.height(),
                    info.frame_len(),
                ));
            }))
            .is_none()
        );

        let mut data = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let mut info = sys::MV_FRAME_OUT_INFO_EX {
            nWidth: 1,
            nHeight: 1,
            nFrameLen: 1,
            nExtendWidth: 4,
            nExtendHeight: 2,
            nFrameLenEx: data.len() as u64,
            enPixelType: PixelType::MONO8.raw() as i32,
            ..Default::default()
        };

        // SAFETY: data, metadata, and the matching callback slot all remain
        // live for the complete synchronous trampoline invocation.
        unsafe { image_trampoline(data.as_mut_ptr(), &mut info, slot.user_data()) };

        assert_eq!(*observed.lock().unwrap(), Some((8, 4, 2, 8)));
        drop_callback_safely(slot.deactivate().expect("active callback"));
    }

    // 验证 image trampoline 在构造 slice 前忽略非法 native buffer。
    #[test]
    fn image_trampoline_ignores_invalid_frame_buffers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let slot = NativeSlot::<ImageCallbackFn>::new();
        assert!(
            slot.activate(Box::new(move |_| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
            }))
            .is_none()
        );

        let mut data = [0_u8; 1];
        let mut info = sys::MV_FRAME_OUT_INFO_EX {
            nFrameLenEx: 1,
            ..Default::default()
        };

        // SAFETY: metadata and the callback slot remain live; the invalid data
        // pointers exercise checks that run before any slice is constructed.
        unsafe { image_trampoline(std::ptr::null_mut(), &mut info, slot.user_data()) };

        info.nFrameLenEx = isize::MAX as u64 + 1;
        // SAFETY: data, metadata, and the callback slot remain live; the
        // oversized range is rejected before the data pointer is dereferenced.
        unsafe { image_trampoline(data.as_mut_ptr(), &mut info, slot.user_data()) };

        info.nFrameLenEx = 1;
        // SAFETY: metadata and the callback slot remain live; the wrapping
        // range is rejected before the synthetic data pointer is dereferenced.
        unsafe { image_trampoline(usize::MAX as *mut u8, &mut info, slot.user_data()) };

        assert_eq!(calls.load(Ordering::Relaxed), 0);
        drop_callback_safely(slot.deactivate().expect("active callback"));
    }

    // 验证 callback slot 替换 closure 时地址稳定，停用后屏蔽晚到调用。
    #[test]
    fn slot_lifecycle_keeps_its_address_and_silences_late_calls() {
        let drops = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let slot = NativeSlot::<ExceptionCallbackFn>::new();
        let user = slot.user_data();

        let first_probe = DropProbe(Arc::clone(&drops));
        assert!(
            slot.activate(Box::new(move |_| {
                let _ = &first_probe;
            }))
            .is_none()
        );

        let second_probe = DropProbe(Arc::clone(&drops));
        let callback_calls = Arc::clone(&calls);
        let previous = slot
            .activate(Box::new(move |_| {
                let _ = &second_probe;
                callback_calls.fetch_add(1, Ordering::Relaxed);
            }))
            .expect("replacement returns the old closure");
        drop_callback_safely(previous);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(slot.user_data(), user);

        drop_callback_safely(slot.deactivate().expect("active callback"));
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(slot.user_data(), user);

        // SAFETY: user points to the still-live slot of the matching type.
        unsafe { exception_trampoline(1, slot.user_data()) };
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    // 验证停用会等待 in-flight callback，随后屏蔽晚到调用。
    #[test]
    fn deactivation_waits_for_in_flight_callback_then_silences_late_calls() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let slot = NativeSlot::<ExceptionCallbackFn>::new();
        let callback_calls = Arc::clone(&calls);
        assert!(
            slot.activate(Box::new(move |_| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }))
            .is_none()
        );
        let user = slot.user_data() as usize;

        std::thread::scope(|scope| {
            scope.spawn(move || {
                // SAFETY: the scoped thread finishes before the slot drops.
                unsafe { exception_trampoline(1, user as *mut c_void) };
            });
            entered_rx.recv().unwrap();

            let (started_tx, started_rx) = mpsc::sync_channel(0);
            let (finished_tx, finished_rx) = mpsc::sync_channel(0);
            let slot = Arc::clone(&slot.slot);
            scope.spawn(move || {
                started_tx.send(()).unwrap();
                let callback = slot.deactivate();
                if let Some(callback) = callback {
                    drop_callback_safely(callback);
                }
                finished_tx.send(()).unwrap();
            });

            started_rx.recv().unwrap();
            assert!(matches!(
                finished_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release_tx.send(()).unwrap();
            finished_rx.recv().unwrap();
        });

        // SAFETY: the slot remains allocated after deactivation.
        unsafe { exception_trampoline(2, slot.user_data()) };
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    // 验证同一 slot 拒绝重入以防 FnMut deadlock，不同 slot 仍可嵌套。
    #[test]
    fn reentry_is_rejected_per_slot_but_other_slots_may_nest() {
        let outer_calls = Arc::new(AtomicUsize::new(0));
        let inner_calls = Arc::new(AtomicUsize::new(0));
        let inner = NativeSlot::<ExceptionCallbackFn>::new();
        let counted_inner = Arc::clone(&inner_calls);
        assert!(
            inner
                .activate(Box::new(move |_| {
                    counted_inner.fetch_add(1, Ordering::Relaxed);
                }))
                .is_none()
        );
        let inner_user = inner.user_data() as usize;

        let address = Arc::new(AtomicUsize::new(0));
        let outer = NativeSlot::<ExceptionCallbackFn>::new();
        let callback_calls = Arc::clone(&outer_calls);
        let callback_address = Arc::clone(&address);
        assert!(
            outer
                .activate(Box::new(move |_| {
                    callback_calls.fetch_add(1, Ordering::Relaxed);
                    let user = callback_address.load(Ordering::Relaxed) as *mut c_void;
                    // SAFETY: the test stores the live matching slot address.
                    unsafe { exception_trampoline(2, user) };
                    // SAFETY: inner_user points to the other live slot.
                    unsafe { exception_trampoline(3, inner_user as *mut c_void) };
                }))
                .is_none()
        );
        address.store(outer.user_data() as usize, Ordering::Relaxed);

        // SAFETY: user points to the live matching slot.
        unsafe { exception_trampoline(1, outer.user_data()) };
        assert_eq!(outer_calls.load(Ordering::Relaxed), 1);
        assert_eq!(inner_calls.load(Ordering::Relaxed), 1);
        drop_callback_safely(outer.deactivate().expect("active callback"));
        drop_callback_safely(inner.deactivate().expect("active callback"));
    }

    // 验证 closure、panic payload 与 destructor panic 都不会跨 FFI unwind。
    #[test]
    fn callback_and_destructor_panics_are_contained() {
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_drops = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        let callback_probe = DropProbe(Arc::clone(&callback_drops));
        let slot = NativeSlot::<ExceptionCallbackFn>::new();
        assert!(
            slot.activate(Box::new(move |_| {
                let _ = &callback_probe;
                callback_calls.fetch_add(1, Ordering::Relaxed);
                panic!("first call panics");
            }))
            .is_none()
        );

        // SAFETY: user points to the live matching slot.
        unsafe { exception_trampoline(1, slot.user_data()) };
        // SAFETY: the slot backing remains live, but the panicking closure has
        // been disabled and removed.
        unsafe { exception_trampoline(2, slot.user_data()) };
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(callback_drops.load(Ordering::Relaxed), 1);
        assert!(!slot.is_active());
        assert!(slot.deactivate().is_none());

        let recovered_calls = Arc::new(AtomicUsize::new(0));
        let recovered_callback_calls = Arc::clone(&recovered_calls);
        assert!(
            slot.activate(Box::new(move |_| {
                recovered_callback_calls.fetch_add(1, Ordering::Relaxed);
            }))
            .is_none()
        );
        // SAFETY: reactivation installed a new closure in the same live slot.
        unsafe { exception_trampoline(3, slot.user_data()) };
        assert_eq!(recovered_calls.load(Ordering::Relaxed), 1);
        drop_callback_safely(slot.deactivate().expect("active callback"));

        let drops = Arc::new(AtomicUsize::new(0));
        log_callback_panic("Test", Box::new(DropProbe(Arc::clone(&drops))));
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        let payload_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            log_callback_panic("Test", Box::new(PanicOnDrop));
        }));
        assert!(payload_result.is_ok());

        let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop_callback_safely(PanicOnDrop);
        }));
        assert!(callback_result.is_ok());
    }
}
