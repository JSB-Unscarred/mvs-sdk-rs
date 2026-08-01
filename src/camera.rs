//! Opened camera — the central public resource type.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::backend;
use crate::callback::EventInfo;
use crate::frame::{Frame, FrameGuard};
use crate::library::{CameraLease, Sdk};
use crate::{AccessMode, CleanupError, EnumNode, FloatNode, IntNode, MvsError, MvsResult};

pub(crate) type ImageCallback = Box<dyn FnMut(&Frame<'_>) + Send + 'static>;
pub(crate) type ExceptionCallback = Box<dyn FnMut(u32) + Send + 'static>;
pub(crate) type EventCallback = Box<dyn FnMut(&EventInfo<'_>) + Send + 'static>;

/// An opened MVS camera.
///
/// Native-operation uncertainty is tracked locally. A failed acquisition
/// transition can be reconciled by retrying [`Camera::stop_grabbing`], while a
/// failed callback registration transition can be reconciled by retrying that
/// callback's unregister operation. Unrelated operations remain available as
/// long as the native handle is live.
///
/// `Camera` is `Send` but not `Sync`; concurrent access to one camera requires
/// external synchronization. Dropping it performs best-effort cleanup and
/// discards cleanup failures, so prefer [`Camera::close`] when those failures
/// must be observed.
///
pub struct Camera {
    inner: backend::Camera,
    lease: CameraLease,
    _not_sync: PhantomData<Cell<()>>,
}

// SAFETY: the native SDK permits moving a handle between threads. Operations
// that mutate Rust-managed state require `&mut self`, and `Cell` keeps !Sync.
unsafe impl Send for Camera {}

impl Camera {
    pub(crate) fn open(
        device: backend::DeviceInfo,
        library: &Arc<Sdk>,
        mode: AccessMode,
    ) -> MvsResult<Self> {
        let pending = library.begin_camera_open();
        match backend::Camera::open(device, mode) {
            Ok(inner) => Ok(Self {
                inner,
                lease: pending.opened(),
                _not_sync: PhantomData,
            }),
            Err(failure) => {
                let backend::OpenFailure {
                    error,
                    rollback_error,
                    disposition,
                } = failure;
                let orphaned = matches!(disposition, Some(backend::HandleDisposition::Orphaned));
                pending.failed(orphaned);
                match rollback_error {
                    Some(destroy) => Err(MvsError::OpenRollback {
                        open: Box::new(error),
                        destroy: Box::new(destroy),
                    }),
                    None => Err(error),
                }
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), CleanupError> {
        let report = self.inner.cleanup();
        if let Some(disposition) = report.disposition {
            self.lease
                .settle(disposition == backend::HandleDisposition::Destroyed);
        }
        report.result
    }

    pub fn as_raw_handle(&self) -> *mut c_void {
        self.inner.as_raw_handle()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// Start image acquisition in callback or polling mode.
    ///
    /// An active image callback selects callback mode; otherwise polling mode
    /// is selected. The mode remains fixed until [`Camera::stop_grabbing`]
    /// succeeds. If start or stop fails, retry stop to reconcile the uncertain
    /// acquisition state.
    pub fn start_grabbing(&mut self) -> MvsResult<()> {
        self.inner.start_grabbing()
    }

    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.inner.stop_grabbing()
    }

    /// Poll for one image while acquisition is running in polling mode.
    ///
    /// Calling this in callback mode returns [`MvsError::CallOrder`].
    /// Multiple guards may coexist, up to the SDK's configured image-node
    /// count. Each guard keeps the camera borrowed until it is released.
    ///
    /// [`MvsError::CallOrder`]: crate::MvsError::CallOrder
    pub fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        self.inner.get_image_buffer(timeout_ms).map(FrameGuard::new)
    }

    /// Register an image callback that may be invoked by the SDK's streaming
    /// thread.
    ///
    /// Registration and replacement require acquisition to be stopped. Stop,
    /// update the callback, and start again to switch acquisition modes.
    ///
    /// The callback must be `Send`; a thread-local `Rc` capture is rejected:
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use mvs_sdk_rs::Camera;
    ///
    /// fn register_non_send(camera: &mut Camera) {
    ///     let state = Rc::new(());
    ///     let _ = camera.register_image_callback(move |_| {
    ///         drop(Rc::clone(&state));
    ///     });
    /// }
    /// ```
    pub fn register_image_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(&Frame<'_>) + Send + 'static,
    {
        self.inner.register_image_callback(Box::new(f))
    }

    /// Unregister the current image callback.
    ///
    /// Acquisition must be stopped before unregistering the callback.
    ///
    /// This waits for an in-flight closure and silences later native calls. A
    /// native failure leaves only this registration uncertain; unregister may
    /// be retried, and cleanup will conservatively retry it as well.
    pub fn unregister_image_callback(&mut self) -> MvsResult<()> {
        self.inner.unregister_image_callback()
    }

    /// Register a device-exception callback.
    ///
    /// The callback must be `Send`; a thread-local `Rc` capture is rejected:
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use mvs_sdk_rs::Camera;
    ///
    /// fn register_non_send(camera: &mut Camera) {
    ///     let state = Rc::new(());
    ///     let _ = camera.register_exception_callback(move |_| {
    ///         drop(Rc::clone(&state));
    ///     });
    /// }
    /// ```
    pub fn register_exception_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(u32) + Send + 'static,
    {
        self.inner.register_exception_callback(Box::new(f))
    }

    /// Unregister the current device-exception callback.
    ///
    /// This waits for an in-flight closure and silences later native calls. A
    /// native failure leaves only this registration uncertain; unregister may
    /// be retried, and cleanup will conservatively retry it as well.
    pub fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        self.inner.unregister_exception_callback()
    }

    /// Register a callback for a named GenICam event.
    ///
    /// The callback must be `Send`; a thread-local `Rc` capture is rejected:
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use mvs_sdk_rs::Camera;
    ///
    /// fn register_non_send(camera: &mut Camera) {
    ///     let state = Rc::new(());
    ///     let _ = camera.register_event_callback("ExposureEnd", move |_| {
    ///         drop(Rc::clone(&state));
    ///     });
    /// }
    /// ```
    pub fn register_event_callback<F>(&mut self, event_name: &str, f: F) -> MvsResult<()>
    where
        F: FnMut(&EventInfo<'_>) + Send + 'static,
    {
        self.inner.register_event_callback(event_name, Box::new(f))
    }

    /// Unregister the callback for one named GenICam event.
    ///
    /// This is distinct from [`Camera::event_notification_off`], which stops
    /// device-side event notification without removing the callback.
    /// Returning from this method means the Rust closure is no longer running
    /// and later native calls through the retained slot are ignored. A native
    /// failure leaves only this named registration uncertain and may be retried.
    pub fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        self.inner.unregister_event_callback(event_name)
    }

    pub fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_on(event_name)
    }

    /// Disable device-side notification for one named GenICam event.
    ///
    /// The event callback remains registered. Use
    /// [`Camera::unregister_event_callback`] to remove it.
    pub fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_off(event_name)
    }

    pub fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        self.inner.set_int(key, value)
    }

    pub fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.inner.get_int(key)
    }

    pub fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        self.inner.get_int_range(key)
    }

    pub fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        self.inner.set_float(key, value)
    }

    pub fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.inner.get_float(key)
    }

    pub fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        self.inner.get_float_range(key)
    }

    pub fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        self.inner.set_bool(key, value)
    }

    pub fn get_bool(&self, key: &str) -> MvsResult<bool> {
        self.inner.get_bool(key)
    }

    pub fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_enum(key, value)
    }

    pub fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_string(key, value)
    }

    pub fn exec_command(&self, key: &str) -> MvsResult<()> {
        self.inner.exec_command(key)
    }

    pub fn get_string(&self, key: &str) -> MvsResult<String> {
        self.inner.get_string(key)
    }

    pub fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.inner.get_enum(key)
    }

    pub fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
        self.inner.get_enum_info(key)
    }

    pub fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        self.inner.set_enum_value(key, value)
    }

    /// Consume the camera and report every cleanup failure.
    ///
    /// Cleanup first silences and drains Rust callbacks, then attempts to stop
    /// acquisition, unregister every native callback, close the device, and
    /// destroy the handle. A failed step does not short-circuit later steps,
    /// and [`CleanupError`] retains failures in call order.
    /// Consequently, an error does not imply that the handle is still alive:
    /// destruction may already have succeeded after an earlier failure.
    /// Calling `close` from this camera's image callback cannot release the
    /// frame that callback is still borrowing; event callbacks are treated
    /// conservatively as well. Those contexts skip native teardown and report
    /// `CleanupStep::DrainCallbacks` with `MvsError::CallOrder`. The SDK does
    /// explicitly support closing and destroying a disconnected handle from
    /// its exception callback, so that context performs only `CloseDevice`
    /// followed by `DestroyHandle`; the current callback slot stays alive
    /// until its trampoline returns.
    ///
    /// This is preferred over relying on [`Drop`], which uses the same cleanup
    /// path but cannot report its result.
    pub fn close(mut self) -> Result<(), CleanupError> {
        self.cleanup()
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (state, acquisition_mode, image_cb, exception_cb, event_cbs) =
            self.inner.debug_details();
        let mut debug = f.debug_struct("Camera");
        debug
            .field("handle", &self.as_raw_handle())
            .field("state", &state);
        if let Some(mode) = acquisition_mode {
            debug.field("acquisition_mode", &mode);
        }
        debug
            .field("image_cb", &image_cb)
            .field("exception_cb", &exception_cb)
            .field("event_cbs", &event_cbs)
            .finish()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
