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

    /// Borrow the opaque native camera handle for advanced interoperability.
    ///
    /// The pointer is owned by this `Camera` and is valid only while its native
    /// handle remains live. Do not close, destroy, or retain it beyond the
    /// camera's lifetime. Calling the vendor API through this pointer is
    /// `unsafe`: changing acquisition, callback, or lifetime state behind this
    /// wrapper can invalidate the assumptions used by later safe methods.
    pub fn as_raw_handle(&self) -> *mut c_void {
        self.inner.as_raw_handle()
    }

    /// Query whether the opened device is still connected.
    ///
    /// This is a diagnostic snapshot; the connection may change immediately
    /// after the call.
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

    /// Stop image acquisition.
    ///
    /// Call this before registering, replacing, or unregistering an image
    /// callback. It may also be retried after an uncertain start/stop failure
    /// to reconcile the wrapper with native acquisition state.
    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.inner.stop_grabbing()
    }

    /// Poll for one image while acquisition is running in polling mode.
    ///
    /// Calling this in callback mode returns [`MvsError::CallOrder`].
    /// Multiple guards may coexist, up to the SDK's configured image-node
    /// count. Each guard keeps the camera borrowed until it is released or
    /// dropped. `timeout_ms` is passed to the SDK in milliseconds.
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
    /// Registering again replaces the Rust closure for this camera. Each
    /// borrowed [`Frame`] is valid only for that invocation; call
    /// [`Frame::to_owned`] before sending image data elsewhere.
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

    /// Register or replace a device-exception callback.
    ///
    /// The callback may run on an SDK-managed thread and receives the vendor's
    /// raw exception message type. Keep it short and hand off longer work.
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

    /// Register or replace a callback for a named GenICam event.
    ///
    /// Call [`Camera::event_notification_on`] separately when device-side
    /// notification is not already enabled. Event metadata is borrowed for the
    /// callback invocation only.
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
    /// Returning successfully means the Rust closure is no longer running and
    /// will not be invoked again. A native failure leaves only this named
    /// registration uncertain and the unregister operation may be retried.
    pub fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        self.inner.unregister_event_callback(event_name)
    }

    /// Enable device-side notification for one named GenICam event.
    ///
    /// This does not register a Rust callback; use
    /// [`Camera::register_event_callback`] for delivery to Rust code.
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

    /// Set an integer GenICam node, such as `Width`, `Height`, or `OffsetX`.
    pub fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        self.inner.set_int(key, value)
    }

    /// Read the current value of an integer GenICam node.
    pub fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.inner.get_int(key)
    }

    /// Read an integer node's current value, bounds, and increment.
    pub fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        self.inner.get_int_range(key)
    }

    /// Set a floating-point GenICam node, such as `ExposureTime` or `Gain`.
    pub fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        self.inner.set_float(key, value)
    }

    /// Read the current value of a floating-point GenICam node.
    pub fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.inner.get_float(key)
    }

    /// Read a floating-point node's current value and bounds.
    pub fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        self.inner.get_float_range(key)
    }

    /// Set a boolean GenICam node, such as `ReverseX`.
    pub fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        self.inner.set_bool(key, value)
    }

    /// Read a boolean GenICam node.
    pub fn get_bool(&self, key: &str) -> MvsResult<bool> {
        self.inner.get_bool(key)
    }

    /// Set an enum GenICam node by symbolic name.
    ///
    /// For example, `set_enum("TriggerMode", "Off")`.
    pub fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_enum(key, value)
    }

    /// Set a string GenICam node, such as `DeviceUserID`.
    pub fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_string(key, value)
    }

    /// Execute a command GenICam node, such as `TriggerSoftware`.
    pub fn exec_command(&self, key: &str) -> MvsResult<()> {
        self.inner.exec_command(key)
    }

    /// Read a string GenICam node.
    ///
    /// The Windows backend reads at most the SDK field capacity and decodes
    /// invalid UTF-8 lossily.
    pub fn get_string(&self, key: &str) -> MvsResult<String> {
        self.inner.get_string(key)
    }

    /// Read an enum GenICam node's current numeric value.
    pub fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.inner.get_enum(key)
    }

    /// Read an enum node's current numeric value and supported-value list.
    ///
    /// This uses the SDK's standard enum query, which reports at most 64
    /// supported values.
    pub fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
        self.inner.get_enum_info(key)
    }

    /// Set an enum GenICam node by its numeric value.
    ///
    /// Prefer [`Camera::set_enum`] when a stable symbolic name is available.
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
    /// followed by `DestroyHandle`.
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
