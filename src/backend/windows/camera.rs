//! Opened camera — the central type of the crate.
//!
//! A [`Camera`] owns an SDK handle and all registered closure-based callbacks.
//! Dropping it stops grabbing, closes the device, and destroys the handle
//! (in that order). Parameter access uses the SDK's native string-keyed API
//! verbatim: `cam.set_int("ExposureTime", 10000)?`.

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int, c_void};

use crate::backend::{AcquisitionMode, CameraState};
use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::error::check;
use crate::sys;
use crate::{
    AccessMode, CleanupError, CleanupFailure, CleanupStep, EnumNode, FloatNode, IntNode, MvsError,
    MvsResult,
};

use super::callback::{
    CallbackSlot, drop_callback_safely, event_trampoline, exception_trampoline, image_trampoline,
};
use super::{DeviceInfo, FrameGuard};

// ---------------------------------------------------------------------------
// AccessMode
// ---------------------------------------------------------------------------

impl AccessMode {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Exclusive => sys::MV_ACCESS_Exclusive,
            Self::ExclusiveWithSwitch => sys::MV_ACCESS_ExclusiveWithSwitch,
            Self::Control => sys::MV_ACCESS_Control,
            Self::ControlWithSwitch => sys::MV_ACCESS_ControlWithSwitch,
            Self::ControlSwitchEnable => sys::MV_ACCESS_ControlSwitchEnable,
            Self::ControlSwitchEnableWithKey => sys::MV_ACCESS_ControlSwitchEnableWithKey,
            Self::Monitor => sys::MV_ACCESS_Monitor,
        }
    }
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// At most one registration can be uncertain because its failure immediately
/// faults the camera and blocks subsequent registration attempts.
#[derive(Clone, Copy, Debug, PartialEq)]
enum UncertainRegistration {
    Image,
    Exception,
    Event(usize),
}

struct CallbackRecord<C> {
    slot: Box<CallbackSlot<C>>,
    native_registered: bool,
}

impl<C> CallbackRecord<C> {
    fn new() -> Self {
        Self {
            slot: Box::new(CallbackSlot::new()),
            native_registered: false,
        }
    }

    fn user_data(&self) -> *mut c_void {
        self.slot.user_data()
    }

    fn is_active(&self) -> bool {
        self.slot.is_active()
    }
}

struct EventRecord {
    name: CString,
    callback: CallbackRecord<EventCallbackFn>,
}

struct CallbackFns<C> {
    image: fn(&mut C, *mut c_void, sys::MvImageCallbackEx, *mut c_void) -> c_int,
    exception: fn(&mut C, *mut c_void, sys::MvExceptionCallback, *mut c_void) -> c_int,
    event: fn(&mut C, *mut c_void, *const c_char, sys::MvEventCallback, *mut c_void) -> c_int,
}

impl CallbackFns<()> {
    const NATIVE: Self = Self {
        image: native_image_callback,
        exception: native_exception_callback,
        event: native_event_callback,
    };
}

fn native_image_callback(
    _: &mut (),
    handle: *mut c_void,
    callback: sys::MvImageCallbackEx,
    user: *mut c_void,
) -> c_int {
    // SAFETY: the caller upholds the SDK callback registration contract.
    unsafe { sys::MV_CC_RegisterImageCallBackEx(handle, callback, user) }
}

fn native_exception_callback(
    _: &mut (),
    handle: *mut c_void,
    callback: sys::MvExceptionCallback,
    user: *mut c_void,
) -> c_int {
    // SAFETY: the caller upholds the SDK callback registration contract.
    unsafe { sys::MV_CC_RegisterExceptionCallBack(handle, callback, user) }
}

fn native_event_callback(
    _: &mut (),
    handle: *mut c_void,
    event_name: *const c_char,
    callback: sys::MvEventCallback,
    user: *mut c_void,
) -> c_int {
    // SAFETY: the caller keeps the event name and user slot alive until
    // successful native handle destruction.
    unsafe { sys::MV_CC_RegisterEventCallBackEx(handle, event_name, callback, user) }
}

struct CleanupFns<C> {
    stop_grabbing: fn(&mut C, *mut c_void) -> c_int,
    unregister_image_callback: fn(&mut C, *mut c_void) -> c_int,
    unregister_exception_callback: fn(&mut C, *mut c_void) -> c_int,
    unregister_event_callback: fn(&mut C, *mut c_void, *const c_char) -> c_int,
    close_device: fn(&mut C, *mut c_void) -> c_int,
    destroy_handle: fn(&mut C, *mut c_void) -> c_int,
}

impl CleanupFns<()> {
    const NATIVE: Self = Self {
        stop_grabbing: native_stop_grabbing,
        unregister_image_callback: native_unregister_image_callback,
        unregister_exception_callback: native_unregister_exception_callback,
        unregister_event_callback: native_unregister_event_callback,
        close_device: native_close_device,
        destroy_handle: native_destroy_handle,
    };
}

fn native_stop_grabbing(_: &mut (), handle: *mut c_void) -> c_int {
    unsafe { sys::MV_CC_StopGrabbing(handle) }
}

fn native_unregister_image_callback(_: &mut (), handle: *mut c_void) -> c_int {
    unsafe { sys::MV_CC_RegisterImageCallBackEx(handle, None, std::ptr::null_mut()) }
}

fn native_unregister_exception_callback(_: &mut (), handle: *mut c_void) -> c_int {
    unsafe { sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut()) }
}

fn native_unregister_event_callback(
    _: &mut (),
    handle: *mut c_void,
    event_name: *const c_char,
) -> c_int {
    unsafe { sys::MV_CC_RegisterEventCallBackEx(handle, event_name, None, std::ptr::null_mut()) }
}

fn native_close_device(_: &mut (), handle: *mut c_void) -> c_int {
    unsafe { sys::MV_CC_CloseDevice(handle) }
}

fn native_destroy_handle(_: &mut (), handle: *mut c_void) -> c_int {
    unsafe { sys::MV_CC_DestroyHandle(handle) }
}

fn record_cleanup_result(
    failures: &mut Vec<CleanupFailure>,
    step: CleanupStep,
    code: c_int,
) -> bool {
    match check(code) {
        Ok(()) => true,
        Err(error) => {
            failures.push(CleanupFailure { step, error });
            false
        }
    }
}

/// An opened MVS camera. `Send` but not `Sync`: the SDK serializes internally,
/// but concurrent calls on the same handle still require external
/// synchronization.
pub(crate) struct Camera {
    handle: Option<*mut c_void>,
    state: CameraState,
    image_cb: Option<CallbackRecord<ImageCallbackFn>>,
    exception_cb: Option<CallbackRecord<ExceptionCallbackFn>>,
    event_cbs: Vec<EventRecord>,
    uncertain_registration: Option<UncertainRegistration>,
}

// SAFETY: the handle is usable from any thread; we just don't allow concurrent
// calls on the same Camera (hence !Sync).
impl Camera {
    pub(crate) fn open(device: DeviceInfo<'_>, mode: AccessMode) -> MvsResult<Self> {
        let mut handle: *mut c_void = std::ptr::null_mut();

        // SAFETY: handle is owned locally until success; dev_info remains valid
        // for the duration of this call (borrowed from DeviceList).
        let code = unsafe { sys::MV_CC_CreateHandle(&mut handle, device.raw()) };
        check(code)?;

        // SAFETY: handle from MV_CC_CreateHandle.
        let code = unsafe { sys::MV_CC_OpenDevice(handle, mode.raw(), 0) };
        if let Err(err) = check(code) {
            // SAFETY: roll back CreateHandle on OpenDevice failure.
            unsafe {
                let _ = sys::MV_CC_DestroyHandle(handle);
            }
            return Err(err);
        }

        Ok(Self {
            handle: Some(handle),
            state: CameraState::Open,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
            uncertain_registration: None,
        })
    }

    fn normal_handle(&self) -> MvsResult<*mut c_void> {
        if !self.state.allows_normal_operations() {
            return Err(MvsError::CallOrder);
        }
        self.handle.ok_or(MvsError::CallOrder)
    }

    fn is_callback_context(&self) -> bool {
        self.image_cb
            .as_ref()
            .is_some_and(|record| record.slot.is_current())
            || self
                .exception_cb
                .as_ref()
                .is_some_and(|record| record.slot.is_current())
            || self
                .event_cbs
                .iter()
                .any(|record| record.callback.slot.is_current())
    }

    fn reject_callback_context(&self) -> MvsResult<()> {
        if self.is_callback_context() {
            Err(MvsError::CallOrder)
        } else {
            Ok(())
        }
    }

    fn handle_in_state(&self, expected: CameraState) -> MvsResult<*mut c_void> {
        if self.state != expected {
            return Err(MvsError::CallOrder);
        }
        self.handle.ok_or(MvsError::CallOrder)
    }

    fn grabbing_handle(&self) -> MvsResult<*mut c_void> {
        if !self.state.is_grabbing() {
            return Err(MvsError::CallOrder);
        }
        self.handle.ok_or(MvsError::CallOrder)
    }

    fn polling_handle(&self) -> MvsResult<*mut c_void> {
        self.handle_in_state(CameraState::Grabbing(AcquisitionMode::Polling))
    }

    fn begin_grabbing(&mut self) -> MvsResult<*mut c_void> {
        self.reject_callback_context()?;
        let handle = self.handle_in_state(CameraState::Open)?;
        let mode = if self
            .image_cb
            .as_ref()
            .is_some_and(CallbackRecord::is_active)
        {
            AcquisitionMode::Callback
        } else {
            AcquisitionMode::Polling
        };
        self.state = CameraState::Grabbing(mode);
        Ok(handle)
    }

    fn fault_on_error(&mut self, result: MvsResult<()>) -> MvsResult<()> {
        if result.is_err() {
            self.state = CameraState::Faulted;
        }
        result
    }

    /// Raw handle, for advanced use-cases.
    pub(crate) fn as_raw_handle(&self) -> *mut c_void {
        self.handle.unwrap_or(std::ptr::null_mut())
    }

    pub(crate) fn is_connected(&self) -> bool {
        let Some(handle) = self.handle else {
            return false;
        };
        // SAFETY: handle was validated at open() and has not been consumed.
        unsafe { sys::MV_CC_IsDeviceConnected(handle) != 0 }
    }

    // ---- Grabbing control ----

    pub(crate) fn start_grabbing(&mut self) -> MvsResult<()> {
        let handle = self.begin_grabbing()?;
        // SAFETY: handle valid.
        let result = check(unsafe { sys::MV_CC_StartGrabbing(handle) });
        self.fault_on_error(result)
    }

    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.grabbing_handle()?;
        // SAFETY: handle valid.
        let result = check(unsafe { sys::MV_CC_StopGrabbing(handle) });
        self.state = CameraState::after_result(&result, CameraState::Open);
        result
    }

    /// Poll for an image, waiting up to `timeout_ms` milliseconds. The
    /// returned [`FrameGuard`] releases the SDK buffer on drop.
    pub(crate) fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        let handle = self.polling_handle()?;
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw is zero-initialized and will be populated by the SDK.
        let code = unsafe { sys::MV_CC_GetImageBuffer(handle, &mut raw, timeout_ms) };
        check(code)?;
        Ok(FrameGuard::new(handle, raw))
    }

    // ---- Callback registration ----

    /// Register an image callback. The closure runs on the SDK's streaming
    /// thread; keep it short or forward the frame through a channel.
    pub(crate) fn register_image_callback(&mut self, f: ImageCallbackFn) -> MvsResult<()> {
        self.register_image_callback_with(f, &CallbackFns::NATIVE, &mut ())
    }

    fn register_image_callback_with<C>(
        &mut self,
        f: ImageCallbackFn,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.handle_in_state(CameraState::Open)?;

        let record = self.image_cb.get_or_insert_with(CallbackRecord::new);
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.native_registered {
            return Ok(());
        }

        let user = record.user_data();
        let code = (fns.image)(context, handle, Some(image_trampoline), user);
        if let Err(error) = check(code) {
            if let Some(callback) = self
                .image_cb
                .as_ref()
                .and_then(|record| record.slot.deactivate())
            {
                drop_callback_safely(callback);
            }
            self.uncertain_registration = Some(UncertainRegistration::Image);
            self.state = CameraState::Faulted;
            return Err(error);
        }
        self.image_cb
            .as_mut()
            .expect("image callback record inserted above")
            .native_registered = true;
        Ok(())
    }

    /// Unregister the image callback (passes `NULL` to the SDK).
    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        self.unregister_image_callback_with(&CallbackFns::NATIVE, &mut ())
    }

    fn unregister_image_callback_with<C>(
        &mut self,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.handle_in_state(CameraState::Open)?;
        let Some(record) = self.image_cb.as_ref() else {
            return Ok(());
        };
        if !record.native_registered {
            return Ok(());
        }

        if let Some(callback) = record.slot.deactivate() {
            drop_callback_safely(callback);
        }
        let result = check((fns.image)(context, handle, None, std::ptr::null_mut()));
        if result.is_ok() {
            self.image_cb
                .as_mut()
                .expect("image callback record checked above")
                .native_registered = false;
        }
        self.fault_on_error(result)
    }

    /// Register an exception callback. Invoked by the SDK on device-level
    /// errors (disconnect, etc.). The argument is the SDK's raw message type.
    pub(crate) fn register_exception_callback(&mut self, f: ExceptionCallbackFn) -> MvsResult<()> {
        self.register_exception_callback_with(f, &CallbackFns::NATIVE, &mut ())
    }

    fn register_exception_callback_with<C>(
        &mut self,
        f: ExceptionCallbackFn,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.normal_handle()?;

        let record = self.exception_cb.get_or_insert_with(CallbackRecord::new);
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.native_registered {
            return Ok(());
        }

        let user = record.user_data();
        let code = (fns.exception)(context, handle, Some(exception_trampoline), user);
        if let Err(error) = check(code) {
            if let Some(callback) = self
                .exception_cb
                .as_ref()
                .and_then(|record| record.slot.deactivate())
            {
                drop_callback_safely(callback);
            }
            self.uncertain_registration = Some(UncertainRegistration::Exception);
            self.state = CameraState::Faulted;
            return Err(error);
        }
        self.exception_cb
            .as_mut()
            .expect("exception callback record inserted above")
            .native_registered = true;
        Ok(())
    }

    /// Unregister the exception callback (passes `NULL` to the SDK).
    pub(crate) fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        self.unregister_exception_callback_with(&CallbackFns::NATIVE, &mut ())
    }

    fn unregister_exception_callback_with<C>(
        &mut self,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.normal_handle()?;
        let Some(record) = self.exception_cb.as_ref() else {
            return Ok(());
        };
        if !record.native_registered {
            return Ok(());
        }

        if let Some(callback) = record.slot.deactivate() {
            drop_callback_safely(callback);
        }
        let result = check((fns.exception)(context, handle, None, std::ptr::null_mut()));
        if result.is_ok() {
            self.exception_cb
                .as_mut()
                .expect("exception callback record checked above")
                .native_registered = false;
        }
        self.fault_on_error(result)
    }

    /// Register an event callback for the named GenICam event (e.g. a custom
    /// trigger or line-state change). Multiple events can be registered; they
    /// are stored independently.
    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        f: EventCallbackFn,
    ) -> MvsResult<()> {
        self.register_event_callback_with(event_name, f, &CallbackFns::NATIVE, &mut ())
    }

    fn register_event_callback_with<C>(
        &mut self,
        event_name: &str,
        f: EventCallbackFn,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;

        let index = match self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str())
        {
            Some(index) => index,
            None => {
                self.event_cbs.push(EventRecord {
                    name,
                    callback: CallbackRecord::new(),
                });
                self.event_cbs.len() - 1
            }
        };

        let record = &self.event_cbs[index].callback;
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.native_registered {
            return Ok(());
        }

        let name_ptr = self.event_cbs[index].name.as_ptr();
        let user = record.user_data();
        let code = (fns.event)(context, handle, name_ptr, Some(event_trampoline), user);
        if let Err(error) = check(code) {
            if let Some(callback) = self.event_cbs[index].callback.slot.deactivate() {
                drop_callback_safely(callback);
            }
            self.uncertain_registration = Some(UncertainRegistration::Event(index));
            self.state = CameraState::Faulted;
            return Err(error);
        }
        self.event_cbs[index].callback.native_registered = true;
        Ok(())
    }

    /// Unregister a callback for one named GenICam event.
    pub(crate) fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        self.unregister_event_callback_with(event_name, &CallbackFns::NATIVE, &mut ())
    }

    fn unregister_event_callback_with<C>(
        &mut self,
        event_name: &str,
        fns: &CallbackFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;
        let index = self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str());
        let Some(index) = index else {
            return Ok(());
        };
        if !self.event_cbs[index].callback.native_registered {
            return Ok(());
        }

        if let Some(callback) = self.event_cbs[index].callback.slot.deactivate() {
            drop_callback_safely(callback);
        }
        let name_ptr = self.event_cbs[index].name.as_ptr();
        let result = check((fns.event)(
            context,
            handle,
            name_ptr,
            None,
            std::ptr::null_mut(),
        ));
        if result.is_ok() {
            self.event_cbs[index].callback.native_registered = false;
        }
        self.fault_on_error(result)
    }

    /// Enable SDK event notification for the named GenICam event.
    pub(crate) fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;
        // SAFETY: handle is valid and name lives for the duration of the call.
        let code = unsafe { sys::MV_CC_EventNotificationOn(handle, name.as_ptr()) };
        check(code)
    }

    /// Disable SDK event notification for the named GenICam event.
    pub(crate) fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;
        // SAFETY: handle is valid and name lives for the duration of the call.
        let code = unsafe { sys::MV_CC_EventNotificationOff(handle, name.as_ptr()) };
        check(code)
    }

    // ---- Parameter access (SDK string-key style) ----

    /// Set an integer node (`MV_CC_SetIntValueEx`). Typical keys: `"Width"`,
    /// `"Height"`, `"OffsetX"`.
    pub(crate) fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        // SAFETY: key points at `k` for the duration of the call.
        let code = unsafe { sys::MV_CC_SetIntValueEx(handle, k.as_ptr(), value) };
        check(code)
    }

    /// Read an integer node (`MV_CC_GetIntValueEx`). Returns the node's
    /// current value; use [`Camera::get_int_range`] if you also need min/max.
    pub(crate) fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.get_int_range(key).map(|v| v.current)
    }

    /// Read an integer node with its full range information.
    pub(crate) fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let mut value = sys::MVCC_INTVALUE_EX::default();
        // SAFETY: value is stack-allocated; key valid for call.
        let code = unsafe { sys::MV_CC_GetIntValueEx(handle, k.as_ptr(), &mut value) };
        check(code)?;
        Ok(IntNode {
            current: value.nCurValue,
            min: value.nMin,
            max: value.nMax,
            inc: value.nInc,
        })
    }

    /// Set a float node (`MV_CC_SetFloatValue`). Typical keys:
    /// `"ExposureTime"`, `"Gain"`, `"AcquisitionFrameRate"`.
    pub(crate) fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetFloatValue(handle, k.as_ptr(), value as c_float) };
        check(code)
    }

    pub(crate) fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.get_float_range(key).map(|v| v.current)
    }

    /// Read a float node with its full range information.
    pub(crate) fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let mut value = sys::MVCC_FLOATVALUE::default();
        // SAFETY: see get_int_range.
        let code = unsafe { sys::MV_CC_GetFloatValue(handle, k.as_ptr(), &mut value) };
        check(code)?;
        Ok(FloatNode {
            current: value.fCurValue,
            min: value.fMin,
            max: value.fMax,
        })
    }

    /// Set a boolean node (`MV_CC_SetBoolValue`). Typical keys:
    /// `"AcquisitionFrameRateEnable"`, `"ReverseX"`.
    pub(crate) fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        // The SDK typedef's C `bool` as `char`; pass 0/1 as i8.
        let v: sys::bool_ = if value { 1 } else { 0 };
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetBoolValue(handle, k.as_ptr(), v) };
        check(code)
    }

    pub(crate) fn get_bool(&self, key: &str) -> MvsResult<bool> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let mut out: sys::bool_ = 0;
        // SAFETY: see get_int.
        let code = unsafe { sys::MV_CC_GetBoolValue(handle, k.as_ptr(), &mut out) };
        check(code)?;
        Ok(out != 0)
    }

    /// Set an enum node by symbolic name (`MV_CC_SetEnumValueByString`).
    /// Example: `cam.set_enum("TriggerMode", "On")`.
    pub(crate) fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: both strings live for the duration of the call.
        let code = unsafe { sys::MV_CC_SetEnumValueByString(handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Set a string node (`MV_CC_SetStringValue`), e.g. `"DeviceUserID"`.
    pub(crate) fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: see set_enum.
        let code = unsafe { sys::MV_CC_SetStringValue(handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Execute a command node (`MV_CC_SetCommandValue`), e.g.
    /// `cam.exec_command("TriggerSoftware")`.
    pub(crate) fn exec_command(&self, key: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetCommandValue(handle, k.as_ptr()) };
        check(code)
    }

    /// Read a string node (`MV_CC_GetStringValue`). Returns up to 255 bytes.
    pub(crate) fn get_string(&self, key: &str) -> MvsResult<String> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let mut value = sys::MVCC_STRINGVALUE::default();
        // SAFETY: value is stack-allocated; key valid for call.
        let code = unsafe { sys::MV_CC_GetStringValue(handle, k.as_ptr(), &mut value) };
        check(code)?;
        let bytes = &value.chCurValue;
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        // SAFETY: c_char is i8 on Windows; reinterpret bytes as u8 for UTF-8.
        let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, end) };
        Ok(String::from_utf8_lossy(slice).into_owned())
    }

    /// Read an enum node's current numeric value (`MV_CC_GetEnumValue`).
    /// See [`Camera::get_enum_info`] to also list supported values.
    pub(crate) fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.get_enum_info(key).map(|v| v.current)
    }

    /// Read an enum node with its supported-values list
    /// (`MV_CC_GetEnumValue`, up to 64 entries — use the SDK's `Ex` variant
    /// yourself for the 256-entry form).
    pub(crate) fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        let mut value = sys::MVCC_ENUMVALUE::default();
        // SAFETY: see get_string.
        let code = unsafe { sys::MV_CC_GetEnumValue(handle, k.as_ptr(), &mut value) };
        check(code)?;
        let n = value.nSupportedNum as usize;
        let supported = value.nSupportValue[..n.min(value.nSupportValue.len())].to_vec();
        Ok(EnumNode {
            current: value.nCurValue,
            supported,
        })
    }

    /// Set an enum node by its numeric value (`MV_CC_SetEnumValue`). Prefer
    /// [`Camera::set_enum`] for symbolic names.
    pub(crate) fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let k = CString::new(key)?;
        // SAFETY: see exec_command.
        let code = unsafe { sys::MV_CC_SetEnumValue(handle, k.as_ptr(), value) };
        check(code)
    }
}

impl Camera {
    pub(crate) fn debug_details(&self) -> (&'static str, Option<&'static str>, bool, bool, usize) {
        (
            self.state.name(),
            self.state.acquisition_mode(),
            self.image_cb
                .as_ref()
                .is_some_and(CallbackRecord::is_active),
            self.exception_cb
                .as_ref()
                .is_some_and(CallbackRecord::is_active),
            self.event_cbs
                .iter()
                .filter(|record| record.callback.is_active())
                .count(),
        )
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), CleanupError> {
        self.cleanup_with(&CleanupFns::NATIVE, &mut ())
    }

    fn cleanup_with<C>(
        &mut self,
        fns: &CleanupFns<C>,
        context: &mut C,
    ) -> Result<(), CleanupError> {
        if self.handle.is_none() {
            self.state = CameraState::Closed;
            return Ok(());
        }

        if self.is_callback_context() {
            return self.abandon_from_callback_context();
        }

        // Revoke admission for every slot before waiting for any one slot.
        // This prevents a busy callback from allowing another callback to
        // enter while cleanup is already under way.
        self.stop_accepting_callbacks();
        self.drain_callbacks();

        // Reserve before consuming the handle so recording an error cannot
        // allocate between native calls. There are at most five non-event
        // attempts, one uncertain event, and one attempt per active event.
        let mut failures = Vec::with_capacity(self.event_cbs.len().saturating_add(6));
        let handle = self.handle.take().expect("handle checked above");

        // Tear down in reverse of open(). A successful DestroyHandle is the
        // SDK's quiescence boundary: after it returns, no new callback may use
        // one of the registered user pointers.
        if self.state.is_grabbing() || self.state == CameraState::Faulted {
            let code = (fns.stop_grabbing)(context, handle);
            record_cleanup_result(&mut failures, CleanupStep::StopGrabbing, code);
        }
        if self
            .image_cb
            .as_ref()
            .is_some_and(|record| record.native_registered)
            || self.uncertain_registration == Some(UncertainRegistration::Image)
        {
            let code = (fns.unregister_image_callback)(context, handle);
            record_cleanup_result(&mut failures, CleanupStep::UnregisterImageCallback, code);
        }
        if self
            .exception_cb
            .as_ref()
            .is_some_and(|record| record.native_registered)
            || self.uncertain_registration == Some(UncertainRegistration::Exception)
        {
            let code = (fns.unregister_exception_callback)(context, handle);
            record_cleanup_result(
                &mut failures,
                CleanupStep::UnregisterExceptionCallback,
                code,
            );
        }
        for (index, record) in self.event_cbs.iter().enumerate() {
            let uncertain =
                self.uncertain_registration == Some(UncertainRegistration::Event(index));
            if record.callback.native_registered || uncertain {
                let code = (fns.unregister_event_callback)(context, handle, record.name.as_ptr());
                record_cleanup_result(&mut failures, CleanupStep::UnregisterEventCallback, code);
            }
        }

        let code = (fns.close_device)(context, handle);
        record_cleanup_result(&mut failures, CleanupStep::CloseDevice, code);

        let code = (fns.destroy_handle)(context, handle);
        let destroyed = record_cleanup_result(&mut failures, CleanupStep::DestroyHandle, code);

        if destroyed {
            self.image_cb = None;
            self.exception_cb = None;
            self.event_cbs.clear();
            self.uncertain_registration = None;
        } else {
            // DestroyHandle failed, so the native side may still retain one
            // or more callback pointers or event-name pointers. Their now
            // empty backing allocations must remain valid indefinitely.
            self.leak_callback_backing();
        }

        // On failure the raw native handle is intentionally leaked as well;
        // taking it above guarantees this wrapper never retries teardown.
        self.state = CameraState::Closed;

        if failures.is_empty() {
            Ok(())
        } else {
            Err(CleanupError::new(failures))
        }
    }

    fn stop_accepting_callbacks(&self) {
        if let Some(record) = &self.image_cb {
            record.slot.stop_accepting();
        }
        if let Some(record) = &self.exception_cb {
            record.slot.stop_accepting();
        }
        for record in &self.event_cbs {
            record.callback.slot.stop_accepting();
        }
    }

    fn drain_callbacks(&self) {
        if let Some(record) = &self.image_cb
            && let Some(callback) = record.slot.take_callback()
        {
            drop_callback_safely(callback);
        }
        if let Some(record) = &self.exception_cb
            && let Some(callback) = record.slot.take_callback()
        {
            drop_callback_safely(callback);
        }
        for record in &self.event_cbs {
            if let Some(callback) = record.callback.slot.take_callback() {
                drop_callback_safely(callback);
            }
        }
    }

    fn try_drain_callbacks(&self) {
        if let Some(record) = &self.image_cb
            && let Some(callback) = record.slot.deactivate_nonblocking()
        {
            drop_callback_safely(callback);
        }
        if let Some(record) = &self.exception_cb
            && let Some(callback) = record.slot.deactivate_nonblocking()
        {
            drop_callback_safely(callback);
        }
        for record in &self.event_cbs {
            if let Some(callback) = record.callback.slot.deactivate_nonblocking() {
                drop_callback_safely(callback);
            }
        }
    }

    fn abandon_from_callback_context(&mut self) -> Result<(), CleanupError> {
        self.stop_accepting_callbacks();
        self.try_drain_callbacks();

        // Native teardown can wait for the callback currently executing this
        // code. Leave the handle and every native-referenced allocation alive
        // instead of deadlocking or freeing the current trampoline's slot.
        let _ = self.handle.take();
        self.leak_callback_backing();
        self.state = CameraState::Closed;

        Err(CleanupError::new(vec![CleanupFailure {
            step: CleanupStep::DrainCallbacks,
            error: MvsError::CallOrder,
        }]))
    }

    fn leak_callback_backing(&mut self) {
        std::mem::forget(self.image_cb.take());
        std::mem::forget(self.exception_cb.take());
        std::mem::forget(std::mem::take(&mut self.event_cbs));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::backend::{AcquisitionMode, CameraState};
    use crate::camera::{
        EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
        ImageCallback as ImageCallbackFn,
    };
    use crate::{CleanupStep, Frame, MvsError, sys};

    use super::{
        CallbackFns, CallbackRecord, Camera, CleanupFns, EventRecord, UncertainRegistration,
    };

    const FULL_CLEANUP_STEPS: [CleanupStep; 7] = [
        CleanupStep::StopGrabbing,
        CleanupStep::UnregisterImageCallback,
        CleanupStep::UnregisterExceptionCallback,
        CleanupStep::UnregisterEventCallback,
        CleanupStep::UnregisterEventCallback,
        CleanupStep::CloseDevice,
        CleanupStep::DestroyHandle,
    ];

    #[derive(Default)]
    struct FakeCleanup {
        calls: Vec<CleanupStep>,
        event_names: Vec<String>,
        results: VecDeque<c_int>,
        drop_probe: Option<Arc<AtomicUsize>>,
        drops_seen: Vec<usize>,
    }

    impl FakeCleanup {
        fn with_results(results: impl IntoIterator<Item = u32>) -> Self {
            Self {
                results: results.into_iter().map(|result| result as c_int).collect(),
                ..Self::default()
            }
        }

        fn with_results_and_probe(
            results: impl IntoIterator<Item = u32>,
            drop_probe: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                drop_probe: Some(drop_probe),
                ..Self::with_results(results)
            }
        }

        fn call(&mut self, step: CleanupStep) -> c_int {
            self.calls.push(step);
            if let Some(probe) = &self.drop_probe {
                self.drops_seen.push(probe.load(Ordering::SeqCst));
            }
            self.results
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected cleanup call: {step}"))
        }
    }

    const FAKE_CLEANUP_FNS: CleanupFns<FakeCleanup> = CleanupFns {
        stop_grabbing: fake_stop_grabbing,
        unregister_image_callback: fake_unregister_image_callback,
        unregister_exception_callback: fake_unregister_exception_callback,
        unregister_event_callback: fake_unregister_event_callback,
        close_device: fake_close_device,
        destroy_handle: fake_destroy_handle,
    };

    fn fake_stop_grabbing(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::StopGrabbing)
    }

    fn fake_unregister_image_callback(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::UnregisterImageCallback)
    }

    fn fake_unregister_exception_callback(
        context: &mut FakeCleanup,
        _handle: *mut c_void,
    ) -> c_int {
        context.call(CleanupStep::UnregisterExceptionCallback)
    }

    fn fake_unregister_event_callback(
        context: &mut FakeCleanup,
        _handle: *mut c_void,
        event_name: *const c_char,
    ) -> c_int {
        assert!(!event_name.is_null());
        // SAFETY: production cleanup passes a live CString owned by EventRecord.
        let event_name = unsafe { CStr::from_ptr(event_name) }
            .to_string_lossy()
            .into_owned();
        context.event_names.push(event_name);
        context.call(CleanupStep::UnregisterEventCallback)
    }

    fn fake_close_device(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::CloseDevice)
    }

    fn fake_destroy_handle(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::DestroyHandle)
    }

    #[derive(Clone, Copy)]
    struct ImageNativeCall {
        callback: sys::MvImageCallbackEx,
        user: usize,
    }

    #[derive(Clone, Copy)]
    struct ExceptionNativeCall {
        callback: sys::MvExceptionCallback,
        user: usize,
    }

    struct EventNativeCall {
        name: String,
        name_ptr: usize,
        callback: sys::MvEventCallback,
        user: usize,
    }

    #[derive(Default)]
    struct FakeCallbacks {
        image_calls: Vec<ImageNativeCall>,
        exception_calls: Vec<ExceptionNativeCall>,
        event_calls: Vec<EventNativeCall>,
        results: VecDeque<c_int>,
        invoke_synchronously: bool,
    }

    impl FakeCallbacks {
        fn with_results(results: impl IntoIterator<Item = u32>) -> Self {
            Self {
                results: results.into_iter().map(|result| result as c_int).collect(),
                ..Self::default()
            }
        }

        fn result(&mut self) -> c_int {
            self.results.pop_front().unwrap_or(sys::MV_OK as c_int)
        }

        fn invoke_image(&self, index: usize) {
            let call = self.image_calls[index];
            let callback = call.callback.expect("expected an image registration");
            let mut info = sys::MV_FRAME_OUT_INFO_EX::default();
            // SAFETY: the recorded user pointer still belongs to the live Camera
            // under test, and the raw frame metadata lives through this call.
            unsafe {
                callback(ptr::null_mut(), &mut info, call.user as *mut c_void);
            }
        }

        fn invoke_exception(&self, index: usize) {
            let call = self.exception_calls[index];
            let callback = call.callback.expect("expected an exception registration");
            // SAFETY: the recorded user pointer still belongs to the live Camera.
            unsafe {
                callback(7, call.user as *mut c_void);
            }
        }

        fn invoke_event(&self, index: usize) {
            let call = &self.event_calls[index];
            let callback = call.callback.expect("expected an event registration");
            let mut info = sys::MV_EVENT_OUT_INFO::default();
            // SAFETY: the recorded user pointer still belongs to the live Camera
            // under test, and the raw event metadata lives through this call.
            unsafe {
                callback(&mut info, call.user as *mut c_void);
            }
        }
    }

    const FAKE_CALLBACK_FNS: CallbackFns<FakeCallbacks> = CallbackFns {
        image: fake_image_callback,
        exception: fake_exception_callback,
        event: fake_event_callback,
    };

    fn fake_image_callback(
        context: &mut FakeCallbacks,
        _handle: *mut c_void,
        callback: sys::MvImageCallbackEx,
        user: *mut c_void,
    ) -> c_int {
        context.image_calls.push(ImageNativeCall {
            callback,
            user: user as usize,
        });
        if context.invoke_synchronously
            && let Some(callback) = callback
        {
            let mut info = sys::MV_FRAME_OUT_INFO_EX::default();
            // SAFETY: registration supplies a live slot and this fake keeps
            // the raw metadata alive until the synchronous callback returns.
            unsafe {
                callback(ptr::null_mut(), &mut info, user);
            }
        }
        context.result()
    }

    fn fake_exception_callback(
        context: &mut FakeCallbacks,
        _handle: *mut c_void,
        callback: sys::MvExceptionCallback,
        user: *mut c_void,
    ) -> c_int {
        context.exception_calls.push(ExceptionNativeCall {
            callback,
            user: user as usize,
        });
        if context.invoke_synchronously
            && let Some(callback) = callback
        {
            // SAFETY: registration supplies a live slot.
            unsafe {
                callback(7, user);
            }
        }
        context.result()
    }

    fn fake_event_callback(
        context: &mut FakeCallbacks,
        _handle: *mut c_void,
        event_name: *const c_char,
        callback: sys::MvEventCallback,
        user: *mut c_void,
    ) -> c_int {
        assert!(!event_name.is_null());
        // SAFETY: registration supplies the live CString owned by EventRecord.
        let name = unsafe { CStr::from_ptr(event_name) }
            .to_string_lossy()
            .into_owned();
        context.event_calls.push(EventNativeCall {
            name,
            name_ptr: event_name as usize,
            callback,
            user: user as usize,
        });
        if context.invoke_synchronously
            && let Some(callback) = callback
        {
            let mut info = sys::MV_EVENT_OUT_INFO::default();
            // SAFETY: registration supplies a live slot and this fake keeps
            // the raw metadata alive until the synchronous callback returns.
            unsafe {
                callback(&mut info, user);
            }
        }
        context.result()
    }

    fn camera(state: CameraState) -> Camera {
        Camera {
            handle: None,
            state,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
            uncertain_registration: None,
        }
    }

    fn camera_with_handle(state: CameraState) -> Camera {
        let mut camera = camera(state);
        camera.handle = Some(ptr::NonNull::<u8>::dangling().as_ptr().cast());
        camera
    }

    fn image_callback() -> ImageCallbackFn {
        Box::new(|_: &Frame<'_>| {})
    }

    fn exception_callback() -> ExceptionCallbackFn {
        Box::new(|_| {})
    }

    fn event_callback() -> EventCallbackFn {
        Box::new(|_| {})
    }

    fn active_record<C>(callback: C) -> CallbackRecord<C> {
        let mut record = CallbackRecord::new();
        assert!(record.slot.activate(callback).is_none());
        record.native_registered = true;
        record
    }

    fn event_record(name: &str, callback: EventCallbackFn) -> EventRecord {
        EventRecord {
            name: CString::new(name).unwrap(),
            callback: active_record(callback),
        }
    }

    fn camera_with_all_callbacks() -> Camera {
        let mut camera = camera_with_handle(CameraState::Grabbing(AcquisitionMode::Callback));
        camera.image_cb = Some(active_record(image_callback()));
        camera.exception_cb = Some(active_record(exception_callback()));
        camera
            .event_cbs
            .push(event_record("ExposureEnd", event_callback()));
        camera
            .event_cbs
            .push(event_record("LineStart", event_callback()));
        camera
    }

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn probed_image_callback(drops: Arc<AtomicUsize>) -> ImageCallbackFn {
        let probe = DropProbe(drops);
        Box::new(move |_: &Frame<'_>| {
            let _ = &probe;
        })
    }

    fn probed_exception_callback(drops: Arc<AtomicUsize>) -> ExceptionCallbackFn {
        let probe = DropProbe(drops);
        Box::new(move |_| {
            let _ = &probe;
        })
    }

    fn probed_event_callback(drops: Arc<AtomicUsize>) -> EventCallbackFn {
        let probe = DropProbe(drops);
        Box::new(move |_| {
            let _ = &probe;
        })
    }

    fn camera_with_probed_callbacks(drops: &Arc<AtomicUsize>) -> Camera {
        let mut camera = camera_with_handle(CameraState::Open);
        camera.image_cb = Some(active_record(probed_image_callback(Arc::clone(drops))));
        camera.exception_cb = Some(active_record(probed_exception_callback(Arc::clone(drops))));
        camera.event_cbs.push(event_record(
            "ExposureEnd",
            probed_event_callback(Arc::clone(drops)),
        ));
        camera
    }

    fn assert_cleanup_calls(mut camera: Camera, expected: &[CleanupStep]) -> FakeCleanup {
        let mut context = FakeCleanup::with_results(vec![sys::MV_OK; expected.len()]);
        camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
            .unwrap();

        assert_eq!(context.calls, expected);
        assert!(context.results.is_empty());
        assert!(camera.handle.is_none());
        assert_eq!(camera.state, CameraState::Closed);
        context
    }

    fn registration_count<T>(calls: &[T], is_registration: impl Fn(&T) -> bool) -> usize {
        calls.iter().filter(|call| is_registration(call)).count()
    }

    #[test]
    fn begin_grabbing_selects_mode_from_the_active_image_slot_only() {
        let mut polling = camera_with_handle(CameraState::Open);
        polling.image_cb = Some(CallbackRecord::new());
        polling.exception_cb = Some(active_record(exception_callback()));
        polling
            .event_cbs
            .push(event_record("ExposureEnd", event_callback()));
        let polling_handle = polling.handle.unwrap();

        assert_eq!(polling.begin_grabbing().unwrap(), polling_handle);
        assert_eq!(
            polling.state,
            CameraState::Grabbing(AcquisitionMode::Polling)
        );

        let mut callback = camera_with_handle(CameraState::Open);
        callback.image_cb = Some(active_record(image_callback()));
        let callback_handle = callback.handle.unwrap();

        assert_eq!(callback.begin_grabbing().unwrap(), callback_handle);
        assert_eq!(
            callback.state,
            CameraState::Grabbing(AcquisitionMode::Callback)
        );

        polling.handle = None;
        callback.handle = None;
    }

    #[test]
    fn grabbing_and_polling_handle_checks_distinguish_modes() {
        for mode in [AcquisitionMode::Callback, AcquisitionMode::Polling] {
            let mut camera = camera_with_handle(CameraState::Grabbing(mode));
            let handle = camera.handle.unwrap();

            assert_eq!(camera.grabbing_handle().unwrap(), handle);
            if mode == AcquisitionMode::Polling {
                assert_eq!(camera.polling_handle().unwrap(), handle);
            } else {
                assert!(matches!(camera.polling_handle(), Err(MvsError::CallOrder)));
            }

            camera.handle = None;
        }
    }

    #[test]
    fn grabbing_rejects_image_callback_changes_without_side_effects() {
        let mut callbacks = FakeCallbacks::default();
        let mut polling = camera_with_handle(CameraState::Grabbing(AcquisitionMode::Polling));

        assert!(matches!(
            polling.register_image_callback_with(
                image_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks
            ),
            Err(MvsError::CallOrder)
        ));
        assert!(polling.image_cb.is_none());
        assert!(matches!(
            polling.unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks),
            Err(MvsError::CallOrder)
        ));
        assert!(callbacks.image_calls.is_empty());

        let old_drops = Arc::new(AtomicUsize::new(0));
        let mut callback = camera_with_handle(CameraState::Grabbing(AcquisitionMode::Callback));
        callback.image_cb = Some(active_record(probed_image_callback(Arc::clone(&old_drops))));
        let old_user = callback.image_cb.as_ref().unwrap().user_data();

        assert!(matches!(
            callback.register_image_callback_with(
                image_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks
            ),
            Err(MvsError::CallOrder)
        ));
        assert!(matches!(
            callback.unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks),
            Err(MvsError::CallOrder)
        ));
        let record = callback.image_cb.as_ref().unwrap();
        assert_eq!(record.user_data(), old_user);
        assert!(record.is_active());
        assert_eq!(old_drops.load(Ordering::SeqCst), 0);
        assert!(callbacks.image_calls.is_empty());

        polling.handle = None;
        callback.handle = None;
    }

    #[test]
    fn exception_and_event_callbacks_do_not_change_acquisition_mode() {
        let state = CameraState::Grabbing(AcquisitionMode::Polling);
        let mut camera = camera_with_handle(state);
        let mut callbacks = FakeCallbacks::default();

        camera
            .register_exception_callback_with(
                exception_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        camera
            .register_event_callback_with(
                "ExposureEnd",
                event_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        assert_eq!(camera.state, state);

        camera
            .unregister_exception_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        camera
            .unregister_event_callback_with("ExposureEnd", &FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        assert_eq!(camera.state, state);
        assert_eq!(callbacks.exception_calls.len(), 2);
        assert_eq!(callbacks.event_calls.len(), 2);

        camera.handle = None;
    }

    #[test]
    fn cleanup_stops_both_grabbing_modes() {
        for mode in [AcquisitionMode::Callback, AcquisitionMode::Polling] {
            assert_cleanup_calls(
                camera_with_handle(CameraState::Grabbing(mode)),
                &[
                    CleanupStep::StopGrabbing,
                    CleanupStep::CloseDevice,
                    CleanupStep::DestroyHandle,
                ],
            );
        }
    }

    #[test]
    fn replacements_reuse_native_registrations_and_stable_slots() {
        let mut camera = camera_with_handle(CameraState::Open);
        let mut callbacks = FakeCallbacks::default();

        let old_image_calls = Arc::new(AtomicUsize::new(0));
        let new_image_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&old_image_calls);
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let image_user = callbacks.image_calls[0].user;
        let counter = Arc::clone(&new_image_calls);
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        assert_eq!(Arc::strong_count(&old_image_calls), 1);
        assert_eq!(
            registration_count(&callbacks.image_calls, |call| call.callback.is_some()),
            1
        );
        assert_eq!(
            camera.image_cb.as_ref().unwrap().user_data() as usize,
            image_user
        );
        callbacks.invoke_image(0);
        assert_eq!(old_image_calls.load(Ordering::SeqCst), 0);
        assert_eq!(new_image_calls.load(Ordering::SeqCst), 1);

        let old_exception_calls = Arc::new(AtomicUsize::new(0));
        let new_exception_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&old_exception_calls);
        camera
            .register_exception_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let exception_user = callbacks.exception_calls[0].user;
        let counter = Arc::clone(&new_exception_calls);
        camera
            .register_exception_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        assert_eq!(Arc::strong_count(&old_exception_calls), 1);
        assert_eq!(
            registration_count(&callbacks.exception_calls, |call| call.callback.is_some()),
            1
        );
        assert_eq!(
            camera.exception_cb.as_ref().unwrap().user_data() as usize,
            exception_user
        );
        callbacks.invoke_exception(0);
        assert_eq!(old_exception_calls.load(Ordering::SeqCst), 0);
        assert_eq!(new_exception_calls.load(Ordering::SeqCst), 1);

        let old_event_calls = Arc::new(AtomicUsize::new(0));
        let new_event_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&old_event_calls);
        camera
            .register_event_callback_with(
                "ExposureEnd",
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let event_user = callbacks.event_calls[0].user;
        let event_name_ptr = callbacks.event_calls[0].name_ptr;
        let counter = Arc::clone(&new_event_calls);
        camera
            .register_event_callback_with(
                "ExposureEnd",
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        assert_eq!(Arc::strong_count(&old_event_calls), 1);
        assert_eq!(
            registration_count(&callbacks.event_calls, |call| call.callback.is_some()),
            1
        );
        assert_eq!(camera.event_cbs.len(), 1);
        assert_eq!(
            camera.event_cbs[0].callback.user_data() as usize,
            event_user
        );
        assert_eq!(camera.event_cbs[0].name.as_ptr() as usize, event_name_ptr);
        callbacks.invoke_event(0);
        assert_eq!(old_event_calls.load(Ordering::SeqCst), 0);
        assert_eq!(new_event_calls.load(Ordering::SeqCst), 1);

        camera.handle = None;
    }

    #[test]
    fn unregister_keeps_records_and_reregister_reuses_native_addresses() {
        let mut camera = camera_with_handle(CameraState::Open);
        let mut callbacks = FakeCallbacks::default();

        camera
            .register_image_callback_with(image_callback(), &FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        let image_user = callbacks.image_calls[0].user;
        camera
            .unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        let image_record = camera.image_cb.as_ref().unwrap();
        assert!(!image_record.native_registered);
        assert!(!image_record.slot.is_active());
        camera
            .register_image_callback_with(image_callback(), &FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        let image_registrations: Vec<_> = callbacks
            .image_calls
            .iter()
            .filter(|call| call.callback.is_some())
            .collect();
        assert_eq!(image_registrations.len(), 2);
        assert!(
            image_registrations
                .iter()
                .all(|call| call.user == image_user)
        );

        camera
            .register_exception_callback_with(
                exception_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let exception_user = callbacks
            .exception_calls
            .iter()
            .find(|call| call.callback.is_some())
            .unwrap()
            .user;
        camera
            .unregister_exception_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        let exception_record = camera.exception_cb.as_ref().unwrap();
        assert!(!exception_record.native_registered);
        assert!(!exception_record.slot.is_active());
        camera
            .register_exception_callback_with(
                exception_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let exception_registrations: Vec<_> = callbacks
            .exception_calls
            .iter()
            .filter(|call| call.callback.is_some())
            .collect();
        assert_eq!(exception_registrations.len(), 2);
        assert!(
            exception_registrations
                .iter()
                .all(|call| call.user == exception_user)
        );

        camera
            .register_event_callback_with(
                "ExposureEnd",
                event_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let first_event = callbacks
            .event_calls
            .iter()
            .find(|call| call.callback.is_some())
            .unwrap();
        let event_user = first_event.user;
        let event_name_ptr = first_event.name_ptr;
        camera
            .unregister_event_callback_with("ExposureEnd", &FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        assert_eq!(camera.event_cbs.len(), 1);
        assert!(!camera.event_cbs[0].callback.native_registered);
        assert!(!camera.event_cbs[0].callback.slot.is_active());
        camera
            .register_event_callback_with(
                "ExposureEnd",
                event_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let event_calls: Vec<_> = callbacks
            .event_calls
            .iter()
            .filter(|call| call.name == "ExposureEnd")
            .collect();
        assert_eq!(event_calls.len(), 3);
        assert!(
            event_calls
                .iter()
                .all(|call| call.name_ptr == event_name_ptr)
        );
        let event_registrations: Vec<_> = event_calls
            .iter()
            .filter(|call| call.callback.is_some())
            .collect();
        assert_eq!(event_registrations.len(), 2);
        assert!(
            event_registrations
                .iter()
                .all(|call| call.user == event_user)
        );

        camera.handle = None;
    }

    #[test]
    fn per_camera_slots_are_isolated_and_stable_across_camera_moves() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let mut callbacks = FakeCallbacks::default();

        let mut first = camera_with_handle(CameraState::Open);
        let counter = Arc::clone(&first_calls);
        first
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        let mut second = camera_with_handle(CameraState::Open);
        let counter = Arc::clone(&second_calls);
        second
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        let first_user = callbacks.image_calls[0].user;
        let second_user = callbacks.image_calls[1].user;
        assert_ne!(first_user, second_user);

        let mut cameras = vec![first, second];
        assert_eq!(
            cameras[0].image_cb.as_ref().unwrap().user_data() as usize,
            first_user
        );
        assert_eq!(
            cameras[1].image_cb.as_ref().unwrap().user_data() as usize,
            second_user
        );

        callbacks.invoke_image(0);
        callbacks.invoke_image(1);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);

        for camera in &mut cameras {
            camera.handle = None;
        }
    }

    #[test]
    fn failed_registration_allows_sync_delivery_then_silences_late_callbacks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&drops));
        let counter = Arc::clone(&calls);
        let mut camera = camera_with_handle(CameraState::Open);
        let mut callbacks = FakeCallbacks::with_results([sys::MV_E_RESOURCE]);
        callbacks.invoke_synchronously = true;

        let result = camera.register_image_callback_with(
            Box::new(move |_| {
                let _ = &probe;
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            &FAKE_CALLBACK_FNS,
            &mut callbacks,
        );

        assert!(matches!(result, Err(MvsError::Resource)));
        assert_eq!(camera.state, CameraState::Faulted);
        assert!(camera.uncertain_registration == Some(UncertainRegistration::Image));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!camera.image_cb.as_ref().unwrap().slot.is_active());

        callbacks.invoke_image(0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        camera.handle = None;
    }

    #[test]
    fn failed_unregister_faults_camera_but_keeps_only_the_stable_slot() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&drops));
        let counter = Arc::clone(&calls);
        let mut camera = camera_with_handle(CameraState::Open);
        let mut callbacks = FakeCallbacks::default();

        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    let _ = &probe;
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let user = callbacks.image_calls[0].user;
        callbacks.results.push_back(sys::MV_E_RESOURCE as c_int);

        assert!(matches!(
            camera.unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks),
            Err(MvsError::Resource)
        ));
        let record = camera.image_cb.as_ref().unwrap();
        assert_eq!(record.user_data() as usize, user);
        assert!(record.native_registered);
        assert!(!record.slot.is_active());
        assert_eq!(camera.state, CameraState::Faulted);
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        callbacks.invoke_image(0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        camera.handle = None;
    }

    #[test]
    fn cleanup_runs_every_step_in_order_and_then_becomes_a_noop() {
        let mut camera = camera_with_all_callbacks();
        let mut context = FakeCleanup::with_results(vec![sys::MV_OK; FULL_CLEANUP_STEPS.len()]);

        camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
            .unwrap();

        assert_eq!(context.calls, FULL_CLEANUP_STEPS);
        assert_eq!(context.event_names, ["ExposureEnd", "LineStart"]);
        assert!(context.results.is_empty());
        assert!(camera.handle.is_none());
        assert_eq!(camera.state, CameraState::Closed);

        let mut unexpected_call = FakeCleanup::default();
        camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut unexpected_call)
            .unwrap();
        assert!(unexpected_call.calls.is_empty());
    }

    #[test]
    fn every_cleanup_step_reports_a_single_failure_without_short_circuiting() {
        for (failed_index, expected_failure_step) in FULL_CLEANUP_STEPS.iter().copied().enumerate()
        {
            let mut camera = camera_with_all_callbacks();
            let results = (0..FULL_CLEANUP_STEPS.len()).map(|index| {
                if index == failed_index {
                    sys::MV_E_RESOURCE
                } else {
                    sys::MV_OK
                }
            });
            let mut context = FakeCleanup::with_results(results);

            let error = camera
                .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
                .unwrap_err();

            assert_eq!(context.calls, FULL_CLEANUP_STEPS);
            assert!(context.results.is_empty());
            assert_eq!(error.failures().len(), 1);
            assert_eq!(error.failures()[0].step, expected_failure_step);
            assert_eq!(
                error.failures()[0].error.raw_code(),
                Some(sys::MV_E_RESOURCE)
            );
            assert!(camera.handle.is_none());
            assert_eq!(camera.state, CameraState::Closed);
        }
    }

    #[test]
    fn cleanup_aggregates_all_failures_in_call_order_and_reaches_destroy() {
        let codes = [
            sys::MV_E_HANDLE,
            sys::MV_E_SUPPORT,
            sys::MV_E_RESOURCE,
            sys::MV_E_GC_TIMEOUT,
            sys::MV_E_BUSY,
            sys::MV_E_PRECONDITION,
            sys::MV_E_VERSION,
        ];
        let mut camera = camera_with_all_callbacks();
        let mut context = FakeCleanup::with_results(codes);

        let error = camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
            .unwrap_err();

        assert_eq!(context.calls, FULL_CLEANUP_STEPS);
        assert_eq!(context.calls.last(), Some(&CleanupStep::DestroyHandle));
        let failures = error.failures();
        assert_eq!(failures.len(), FULL_CLEANUP_STEPS.len());
        for ((failure, expected_step), expected_code) in
            failures.iter().zip(FULL_CLEANUP_STEPS).zip(codes)
        {
            assert_eq!(failure.step, expected_step);
            assert_eq!(failure.error.raw_code(), Some(expected_code));
        }
    }

    #[test]
    fn cleanup_unregisters_all_three_uncertain_registration_kinds() {
        let mut image_camera = camera_with_handle(CameraState::Faulted);
        image_camera.image_cb = Some(CallbackRecord::new());
        image_camera.uncertain_registration = Some(UncertainRegistration::Image);
        assert_cleanup_calls(
            image_camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterImageCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );

        let mut exception_camera = camera_with_handle(CameraState::Faulted);
        exception_camera.exception_cb = Some(CallbackRecord::new());
        exception_camera.uncertain_registration = Some(UncertainRegistration::Exception);
        assert_cleanup_calls(
            exception_camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterExceptionCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );

        let mut event_camera = camera_with_handle(CameraState::Faulted);
        event_camera.event_cbs.push(EventRecord {
            name: CString::new("ExposureEnd").unwrap(),
            callback: CallbackRecord::new(),
        });
        event_camera.uncertain_registration = Some(UncertainRegistration::Event(0));
        let context = assert_cleanup_calls(
            event_camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );
        assert_eq!(context.event_names, ["ExposureEnd"]);
    }

    #[test]
    fn uncertain_event_uses_its_stable_index_after_existing_records() {
        let mut camera = camera_with_handle(CameraState::Faulted);
        camera
            .event_cbs
            .push(event_record("ExposureEnd", event_callback()));
        camera
            .event_cbs
            .push(event_record("LineStart", event_callback()));
        camera.event_cbs.push(EventRecord {
            name: CString::new("FrameStart").unwrap(),
            callback: CallbackRecord::new(),
        });
        camera.uncertain_registration = Some(UncertainRegistration::Event(2));

        let context = assert_cleanup_calls(
            camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );
        assert_eq!(
            context.event_names,
            ["ExposureEnd", "LineStart", "FrameStart"]
        );
    }

    #[test]
    fn cleanup_drains_closures_before_native_calls_for_both_destroy_results() {
        let successful_drops = Arc::new(AtomicUsize::new(0));
        let mut successful_camera = camera_with_probed_callbacks(&successful_drops);
        let mut success =
            FakeCleanup::with_results_and_probe(vec![sys::MV_OK; 5], Arc::clone(&successful_drops));
        successful_camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut success)
            .unwrap();
        assert_eq!(successful_drops.load(Ordering::SeqCst), 3);
        assert!(success.drops_seen.iter().all(|drops| *drops == 3));
        assert!(successful_camera.image_cb.is_none());
        assert!(successful_camera.exception_cb.is_none());
        assert!(successful_camera.event_cbs.is_empty());

        let failed_drops = Arc::new(AtomicUsize::new(0));
        let mut failed_camera = camera_with_probed_callbacks(&failed_drops);
        let mut failure = FakeCleanup::with_results_and_probe(
            [
                sys::MV_OK,
                sys::MV_OK,
                sys::MV_OK,
                sys::MV_OK,
                sys::MV_E_RESOURCE,
            ],
            Arc::clone(&failed_drops),
        );
        let error = failed_camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut failure)
            .unwrap_err();
        assert_eq!(error.failures().len(), 1);
        assert_eq!(error.failures()[0].step, CleanupStep::DestroyHandle);
        assert_eq!(failed_drops.load(Ordering::SeqCst), 3);
        assert!(failure.drops_seen.iter().all(|drops| *drops == 3));
        assert!(failed_camera.image_cb.is_none());
        assert!(failed_camera.exception_cb.is_none());
        assert!(failed_camera.event_cbs.is_empty());

        let mut no_retry = FakeCleanup::default();
        failed_camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut no_retry)
            .unwrap();
        assert!(no_retry.calls.is_empty());
    }

    struct SendCamera(Camera);

    // SAFETY: this mirrors the public Camera Send contract solely for moving a
    // synthetic backend camera into the callback-owned test holder.
    unsafe impl Send for SendCamera {}

    #[test]
    fn callback_context_cleanup_abandons_without_native_teardown() {
        let camera_holder = Arc::new(Mutex::new(None::<SendCamera>));
        let outcome = Arc::new(Mutex::new(None::<(Vec<CleanupStep>, Vec<CleanupStep>)>));
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);

        let mut camera = camera_with_handle(CameraState::Open);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    let SendCamera(mut camera) = holder_for_callback
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                        .expect("camera installed before callback");

                    assert!(matches!(
                        camera.reject_callback_context(),
                        Err(MvsError::CallOrder)
                    ));
                    let mut rejected_native = FakeCallbacks::default();
                    let callback_results = [
                        camera.register_image_callback_with(
                            image_callback(),
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        camera.unregister_image_callback_with(
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        camera.register_exception_callback_with(
                            exception_callback(),
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        camera.unregister_exception_callback_with(
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        camera.register_event_callback_with(
                            "ExposureEnd",
                            event_callback(),
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        camera.unregister_event_callback_with(
                            "ExposureEnd",
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                    ];
                    assert!(
                        callback_results
                            .iter()
                            .all(|result| matches!(result, Err(MvsError::CallOrder)))
                    );
                    assert!(rejected_native.image_calls.is_empty());
                    assert!(rejected_native.exception_calls.is_empty());
                    assert!(rejected_native.event_calls.is_empty());

                    let mut native = FakeCleanup::with_results(std::iter::repeat_n(sys::MV_OK, 8));
                    let error = camera
                        .cleanup_with(&FAKE_CLEANUP_FNS, &mut native)
                        .expect_err("callback-context cleanup must be explicit");
                    let failure_steps = error
                        .failures()
                        .iter()
                        .map(|failure| failure.step)
                        .collect();
                    *outcome_for_callback
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some((native.calls, failure_steps));
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        *camera_holder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SendCamera(camera));
        callbacks.invoke_image(0);

        let (native_calls, failure_steps) = outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("callback recorded cleanup outcome");
        assert!(native_calls.is_empty());
        assert_eq!(failure_steps, [CleanupStep::DrainCallbacks]);
        assert!(
            camera_holder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
    }

    #[test]
    fn faulted_and_closed_states_reject_normal_operations() {
        for state in [CameraState::Faulted, CameraState::Closed] {
            let mut camera = camera(state);
            camera.handle = Some(ptr::NonNull::<u8>::dangling().as_ptr().cast());

            assert!(matches!(camera.normal_handle(), Err(MvsError::CallOrder)));

            camera.handle = None;
        }
    }
}
