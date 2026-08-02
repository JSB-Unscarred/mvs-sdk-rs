//! Opened camera — the central public resource type.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;

use crate::backend;
use crate::callback::EventInfo;
use crate::error::CleanupError;
use crate::frame::{Frame, FrameGuard};
use crate::library::{ActiveSdk, CameraLease};
use crate::{AccessMode, EnumNode, FloatNode, IntNode, MvsError, MvsResult};

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
/// discards cleanup failures. Prefer [`Camera::close`] to observe a failure,
/// or [`Camera::close_detailed`] to inspect every failure.
///
pub struct Camera {
    inner: backend::Camera,
    lease: CameraLease,
    _not_sync: PhantomData<Cell<()>>,
}

impl Camera {
    pub(crate) fn open(
        device: backend::DeviceInfo,
        active: &ActiveSdk,
        mode: AccessMode,
    ) -> MvsResult<Self> {
        let pending = active.begin_camera_open();
        match backend::Camera::open(device, mode) {
            Ok(inner) => Ok(Self {
                inner,
                lease: pending.opened(),
                _not_sync: PhantomData,
            }),
            Err(error) => {
                let orphaned = matches!(&error, MvsError::OpenRollback { .. });
                pending.failed(orphaned);
                Err(error)
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), CleanupError> {
        let result = self.inner.cleanup();
        let destroyed = match &result {
            Ok(()) => true,
            Err(error) => error.native_handle_destroyed(),
        };
        self.lease.settle(destroyed);
        result
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
    /// Call this before first registering or unregistering an image callback,
    /// or before switching between callback and polling modes. An already
    /// registered callback may be replaced while callback acquisition runs.
    /// This method may also be retried after an uncertain start/stop failure to
    /// reconcile the wrapper with native acquisition state.
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
    /// First registration requires acquisition to be stopped. Once callback
    /// acquisition is running, registering again replaces only the Rust
    /// closure and does not call the SDK; replacement waits for an in-flight
    /// invocation to finish. Stop acquisition before switching modes or
    /// unregistering. Each borrowed [`Frame`] is valid only for that
    /// invocation; call [`Frame::to_owned`] before sending image data
    /// elsewhere.
    ///
    /// A panic is caught at the FFI boundary and disables this callback after
    /// that invocation. Register another closure to resume delivery.
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
    /// raw exception message type. Keep it short and hand off longer work. A
    /// panic is caught and disables this callback after that invocation;
    /// register another closure to resume delivery.
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
    /// callback invocation only. A panic is caught and disables this callback
    /// after that invocation; register another closure to resume delivery.
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

    /// Consume the camera and close its native resources.
    ///
    /// Cleanup continues after failures so handle destruction is still
    /// attempted. If multiple operations fail, this returns the first error in
    /// call order. Use [`Camera::close_detailed`] to inspect every failure.
    pub fn close(mut self) -> MvsResult<()> {
        self.cleanup().map_err(CleanupError::into_first_error)
    }

    /// Consume the camera and report every cleanup failure.
    ///
    /// Cleanup first silences and drains Rust callbacks, then attempts to stop
    /// acquisition, unregister every native callback, close the device, and
    /// destroy the handle. A failed step does not short-circuit later steps,
    /// and [`CleanupError`] retains failures in call order.
    /// Consequently, an error does not imply that the handle is still alive:
    /// destruction may already have succeeded after an earlier failure.
    /// Closing from this camera's image callback cannot release the frame that
    /// callback is still borrowing; event callbacks are treated conservatively
    /// as well. Those contexts skip native teardown and report
    /// [`MvsError::CallOrder`] through [`CleanupError::errors`]. The SDK does
    /// explicitly support closing and destroying a disconnected handle from
    /// its exception callback, so that context performs only the supported
    /// close-and-destroy sequence.
    ///
    /// This is preferred when diagnostics need more than the first error
    /// returned by [`Camera::close`]. [`Drop`] uses the same cleanup path but
    /// cannot report its result.
    pub fn close_detailed(mut self) -> Result<(), CleanupError> {
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
