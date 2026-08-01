//! Opened camera — the central type of the crate.
//!
//! A [`Camera`] owns an SDK handle and all registered closure-based callbacks.
//! Dropping it stops grabbing, closes the device, and destroys the handle
//! (in that order). Parameter access uses the SDK's native string-keyed API
//! verbatim: `cam.set_int("ExposureTime", 10000)?`.

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::sync::Mutex;

use crate::backend::CameraState;
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
    CallbackRegistration, EventCallback, ExceptionCallback, ImageCallback, event_trampoline,
    exception_trampoline, image_trampoline,
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
#[derive(Clone, Copy, PartialEq)]
enum UncertainRegistration {
    Image,
    Exception,
    Event(usize),
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
    image_cb: Option<CallbackRegistration<ImageCallback>>,
    retired_image_cbs: Vec<CallbackRegistration<ImageCallback>>,
    exception_cb: Option<CallbackRegistration<ExceptionCallback>>,
    retired_exception_cbs: Vec<CallbackRegistration<ExceptionCallback>>,
    event_cbs: Vec<(CString, CallbackRegistration<EventCallback>)>,
    retired_event_cbs: Vec<(CString, CallbackRegistration<EventCallback>)>,
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
            retired_image_cbs: Vec::new(),
            exception_cb: None,
            retired_exception_cbs: Vec::new(),
            event_cbs: Vec::new(),
            retired_event_cbs: Vec::new(),
            uncertain_registration: None,
        })
    }

    fn normal_handle(&self) -> MvsResult<*mut c_void> {
        if !self.state.allows_normal_operations() {
            return Err(MvsError::CallOrder);
        }
        self.handle.ok_or(MvsError::CallOrder)
    }

    fn handle_in_state(&self, expected: CameraState) -> MvsResult<*mut c_void> {
        if self.state != expected {
            return Err(MvsError::CallOrder);
        }
        self.handle.ok_or(MvsError::CallOrder)
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
        let handle = self.handle_in_state(CameraState::Open)?;
        // SAFETY: handle valid.
        let result = check(unsafe { sys::MV_CC_StartGrabbing(handle) });
        self.state = CameraState::after_result(&result, CameraState::Grabbing);
        result
    }

    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        let handle = self.handle_in_state(CameraState::Grabbing)?;
        // SAFETY: handle valid.
        let result = check(unsafe { sys::MV_CC_StopGrabbing(handle) });
        self.state = CameraState::after_result(&result, CameraState::Open);
        result
    }

    /// Poll for an image, waiting up to `timeout_ms` milliseconds. The
    /// returned [`FrameGuard`] releases the SDK buffer on drop.
    pub(crate) fn get_image_buffer(&mut self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        let handle = self.handle_in_state(CameraState::Grabbing)?;
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw is zero-initialized and will be populated by the SDK.
        let code = unsafe { sys::MV_CC_GetImageBuffer(handle, &mut raw, timeout_ms) };
        check(code)?;
        Ok(FrameGuard::new(handle, raw))
    }

    // ---- Callback registration ----

    /// Register an image callback. The closure runs on the SDK's streaming
    /// thread; keep it short or forward the frame through a channel.
    ///
    /// Replacing the callback while grabbing is active is technically
    /// supported by the SDK, but to be safe call [`Camera::stop_grabbing`]
    /// first.
    pub(crate) fn register_image_callback(&mut self, f: ImageCallbackFn) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        // Ensure every post-FFI move into retired storage is allocation-free.
        self.retired_image_cbs.reserve(1);
        let cb = CallbackRegistration::new(ImageCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: the trampoline has the ABI expected by the SDK, and `user`
        // is a stable Arc token retained by this backend.
        let code =
            unsafe { sys::MV_CC_RegisterImageCallBackEx(handle, Some(image_trampoline), user) };
        if let Err(error) = check(code) {
            // Retain the token until handle destruction even on failure, in
            // case the native API stored pUser before reporting the error.
            self.retired_image_cbs.push(cb);
            self.uncertain_registration = Some(UncertainRegistration::Image);
            self.state = CameraState::Faulted;
            return Err(error);
        }
        if let Some(previous) = self.image_cb.replace(cb) {
            self.retired_image_cbs.push(previous);
        }
        Ok(())
    }

    /// Unregister the image callback (passes `NULL` to the SDK).
    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        if self.image_cb.is_none() {
            return Ok(());
        }
        self.retired_image_cbs.reserve(1);
        // SAFETY: handle valid; passing None + null user to deregister.
        let result = check(unsafe {
            sys::MV_CC_RegisterImageCallBackEx(handle, None, std::ptr::null_mut())
        });
        self.finish_image_unregister(result)
    }

    /// Register an exception callback. Invoked by the SDK on device-level
    /// errors (disconnect, etc.). The argument is the SDK's raw message type.
    pub(crate) fn register_exception_callback(&mut self, f: ExceptionCallbackFn) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        self.retired_exception_cbs.reserve(1);
        let cb = CallbackRegistration::new(ExceptionCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: see register_image_callback.
        let code = unsafe {
            sys::MV_CC_RegisterExceptionCallBack(handle, Some(exception_trampoline), user)
        };
        if let Err(error) = check(code) {
            self.retired_exception_cbs.push(cb);
            self.uncertain_registration = Some(UncertainRegistration::Exception);
            self.state = CameraState::Faulted;
            return Err(error);
        }
        if let Some(previous) = self.exception_cb.replace(cb) {
            self.retired_exception_cbs.push(previous);
        }
        Ok(())
    }

    /// Unregister the exception callback (passes `NULL` to the SDK).
    pub(crate) fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        if self.exception_cb.is_none() {
            return Ok(());
        }
        self.retired_exception_cbs.reserve(1);
        // SAFETY: handle valid; passing None + null user to deregister.
        let result = check(unsafe {
            sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut())
        });
        self.finish_exception_unregister(result)
    }

    /// Register an event callback for the named GenICam event (e.g. a custom
    /// trigger or line-state change). Multiple events can be registered; they
    /// are stored independently.
    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        f: EventCallbackFn,
    ) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;
        self.event_cbs.reserve(1);
        self.retired_event_cbs.reserve(1);
        let cb = CallbackRegistration::new(EventCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: `name` remains alive while registered, and `user` points to
        // a stable Arc token retained by this backend.
        let code = unsafe {
            sys::MV_CC_RegisterEventCallBackEx(handle, name.as_ptr(), Some(event_trampoline), user)
        };
        if let Err(error) = check(code) {
            self.retired_event_cbs.push((name, cb));
            let retired_index = self.retired_event_cbs.len() - 1;
            self.uncertain_registration = Some(UncertainRegistration::Event(retired_index));
            self.state = CameraState::Faulted;
            return Err(error);
        }
        if let Some(index) = self
            .event_cbs
            .iter()
            .position(|(registered, _)| registered.as_c_str() == name.as_c_str())
        {
            self.retired_event_cbs.push(self.event_cbs.remove(index));
        }
        self.event_cbs.push((name, cb));
        Ok(())
    }

    /// Unregister a callback for one named GenICam event.
    pub(crate) fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        let handle = self.normal_handle()?;
        let name = CString::new(event_name)?;
        let active_index = self
            .event_cbs
            .iter()
            .position(|(registered, _)| registered.as_c_str() == name.as_c_str());
        let Some(active_index) = active_index else {
            return Ok(());
        };
        self.retired_event_cbs.reserve(1);
        let name_ptr = self.event_cbs[active_index].0.as_ptr();
        // SAFETY: handle is valid, and name_ptr remains valid for the call.
        let result = check(unsafe {
            sys::MV_CC_RegisterEventCallBackEx(handle, name_ptr, None, std::ptr::null_mut())
        });
        self.finish_event_unregister(active_index, result)
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
    fn finish_image_unregister(&mut self, result: MvsResult<()>) -> MvsResult<()> {
        self.fault_on_error(result)?;
        if let Some(previous) = self.image_cb.take() {
            self.retired_image_cbs.push(previous);
        }
        Ok(())
    }

    fn finish_exception_unregister(&mut self, result: MvsResult<()>) -> MvsResult<()> {
        self.fault_on_error(result)?;
        if let Some(previous) = self.exception_cb.take() {
            self.retired_exception_cbs.push(previous);
        }
        Ok(())
    }

    fn finish_event_unregister(
        &mut self,
        active_index: usize,
        result: MvsResult<()>,
    ) -> MvsResult<()> {
        self.fault_on_error(result)?;
        self.retired_event_cbs
            .push(self.event_cbs.remove(active_index));
        Ok(())
    }

    pub(crate) fn debug_details(&self) -> (&'static str, bool, bool, bool, usize) {
        (
            self.state.name(),
            self.state.is_grabbing(),
            self.image_cb.is_some(),
            self.exception_cb.is_some(),
            self.event_cbs.len(),
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

        // Reserve before consuming the handle so recording an error cannot
        // allocate between native calls. There are at most five non-event
        // attempts, one uncertain event, and one attempt per active event.
        let mut failures = Vec::with_capacity(self.event_cbs.len().saturating_add(6));
        let handle = self.handle.take().expect("handle checked above");

        // Tear down in reverse of open(). A successful DestroyHandle is the
        // SDK's quiescence boundary: after it returns, no new callback may use
        // one of the registered user pointers.
        if matches!(self.state, CameraState::Grabbing | CameraState::Faulted) {
            let code = (fns.stop_grabbing)(context, handle);
            record_cleanup_result(&mut failures, CleanupStep::StopGrabbing, code);
        }
        if self.image_cb.is_some()
            || self.uncertain_registration == Some(UncertainRegistration::Image)
        {
            let code = (fns.unregister_image_callback)(context, handle);
            record_cleanup_result(&mut failures, CleanupStep::UnregisterImageCallback, code);
        }
        if self.exception_cb.is_some()
            || self.uncertain_registration == Some(UncertainRegistration::Exception)
        {
            let code = (fns.unregister_exception_callback)(context, handle);
            record_cleanup_result(
                &mut failures,
                CleanupStep::UnregisterExceptionCallback,
                code,
            );
        }
        for (name, _) in &self.event_cbs {
            let code = (fns.unregister_event_callback)(context, handle, name.as_ptr());
            record_cleanup_result(&mut failures, CleanupStep::UnregisterEventCallback, code);
        }
        if let Some(UncertainRegistration::Event(index)) = self.uncertain_registration {
            // A failed registration can leave its event name only in the
            // retired list even though the native side may have stored it.
            if let Some((name, _)) = self.retired_event_cbs.get(index) {
                let already_unregistered = self
                    .event_cbs
                    .iter()
                    .any(|(active, _)| active.as_c_str() == name.as_c_str());
                if !already_unregistered {
                    let code = (fns.unregister_event_callback)(context, handle, name.as_ptr());
                    record_cleanup_result(
                        &mut failures,
                        CleanupStep::UnregisterEventCallback,
                        code,
                    );
                }
            }
        }

        let code = (fns.close_device)(context, handle);
        record_cleanup_result(&mut failures, CleanupStep::CloseDevice, code);

        let code = (fns.destroy_handle)(context, handle);
        let destroyed = record_cleanup_result(&mut failures, CleanupStep::DestroyHandle, code);

        if !destroyed {
            // DestroyHandle failed, so the native side may still retain one
            // or more callback pointers. Leaking their strong references is
            // safer than freeing memory the SDK could call into later.
            self.leak_callbacks();
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

    fn leak_callbacks(&mut self) {
        std::mem::forget(self.image_cb.take());
        std::mem::forget(std::mem::take(&mut self.retired_image_cbs));
        std::mem::forget(self.exception_cb.take());
        std::mem::forget(std::mem::take(&mut self.retired_exception_cbs));
        std::mem::forget(std::mem::take(&mut self.event_cbs));
        std::mem::forget(std::mem::take(&mut self.retired_event_cbs));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::backend::CameraState;
    use crate::camera::{EventCallback as EventCallbackFn, ImageCallback as ImageCallbackFn};
    use crate::{CleanupStep, Frame, MvsError, sys};

    use super::super::callback::{
        CallbackRegistration, EventCallback, ExceptionCallback, ImageCallback,
    };
    use super::{Camera, CleanupFns, UncertainRegistration};

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
        results: VecDeque<c_int>,
    }

    impl FakeCleanup {
        fn with_results(results: impl IntoIterator<Item = u32>) -> Self {
            Self {
                calls: Vec::new(),
                results: results.into_iter().map(|result| result as c_int).collect(),
            }
        }

        fn call(&mut self, step: CleanupStep) -> c_int {
            self.calls.push(step);
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
        _event_name: *const c_char,
    ) -> c_int {
        context.call(CleanupStep::UnregisterEventCallback)
    }

    fn fake_close_device(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::CloseDevice)
    }

    fn fake_destroy_handle(context: &mut FakeCleanup, _handle: *mut c_void) -> c_int {
        context.call(CleanupStep::DestroyHandle)
    }

    fn camera(state: CameraState) -> Camera {
        Camera {
            handle: None,
            state,
            image_cb: None,
            retired_image_cbs: Vec::new(),
            exception_cb: None,
            retired_exception_cbs: Vec::new(),
            event_cbs: Vec::new(),
            retired_event_cbs: Vec::new(),
            uncertain_registration: None,
        }
    }

    fn camera_with_handle(state: CameraState) -> Camera {
        let mut camera = camera(state);
        camera.handle = Some(std::ptr::NonNull::<u8>::dangling().as_ptr().cast());
        camera
    }

    fn image_callback() -> CallbackRegistration<ImageCallback> {
        let callback: ImageCallbackFn = Box::new(|_: &Frame<'_>| {});
        CallbackRegistration::new(ImageCallback(Mutex::new(callback)))
    }

    fn exception_callback() -> CallbackRegistration<ExceptionCallback> {
        CallbackRegistration::new(ExceptionCallback(Mutex::new(Box::new(|_| {}))))
    }

    fn event_callback() -> CallbackRegistration<EventCallback> {
        let callback: EventCallbackFn = Box::new(|_| {});
        CallbackRegistration::new(EventCallback(Mutex::new(callback)))
    }

    fn camera_with_all_callbacks() -> Camera {
        let mut camera = camera_with_handle(CameraState::Grabbing);
        camera.image_cb = Some(image_callback());
        camera.exception_cb = Some(exception_callback());
        camera
            .event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        camera
            .event_cbs
            .push((CString::new("LineStart").unwrap(), event_callback()));
        camera
    }

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn probed_image_callback(drops: Arc<AtomicUsize>) -> CallbackRegistration<ImageCallback> {
        let probe = DropProbe(drops);
        let callback: ImageCallbackFn = Box::new(move |_: &Frame<'_>| {
            let _ = &probe;
        });
        CallbackRegistration::new(ImageCallback(Mutex::new(callback)))
    }

    fn probed_exception_callback(
        drops: Arc<AtomicUsize>,
    ) -> CallbackRegistration<ExceptionCallback> {
        let probe = DropProbe(drops);
        CallbackRegistration::new(ExceptionCallback(Mutex::new(Box::new(move |_| {
            let _ = &probe;
        }))))
    }

    fn probed_event_callback(drops: Arc<AtomicUsize>) -> CallbackRegistration<EventCallback> {
        let probe = DropProbe(drops);
        let callback: EventCallbackFn = Box::new(move |_| {
            let _ = &probe;
        });
        CallbackRegistration::new(EventCallback(Mutex::new(callback)))
    }

    fn camera_with_probed_callbacks(drops: &Arc<AtomicUsize>) -> Camera {
        let mut camera = camera_with_handle(CameraState::Open);
        camera.image_cb = Some(probed_image_callback(Arc::clone(drops)));
        camera
            .retired_image_cbs
            .push(probed_image_callback(Arc::clone(drops)));
        camera.exception_cb = Some(probed_exception_callback(Arc::clone(drops)));
        camera
            .retired_exception_cbs
            .push(probed_exception_callback(Arc::clone(drops)));
        camera.event_cbs.push((
            CString::new("ExposureEnd").unwrap(),
            probed_event_callback(Arc::clone(drops)),
        ));
        camera.retired_event_cbs.push((
            CString::new("LineStart").unwrap(),
            probed_event_callback(Arc::clone(drops)),
        ));
        camera
    }

    fn assert_cleanup_calls(mut camera: Camera, expected: &[CleanupStep]) {
        let mut context = FakeCleanup::with_results(vec![sys::MV_OK; expected.len()]);
        camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
            .unwrap();

        assert_eq!(context.calls, expected);
        assert!(context.results.is_empty());
        assert!(camera.handle.is_none());
        assert_eq!(camera.state, CameraState::Closed);
    }

    #[test]
    fn cleanup_runs_every_step_in_order_and_then_becomes_a_noop() {
        let mut camera = camera_with_all_callbacks();
        let mut context = FakeCleanup::with_results(vec![sys::MV_OK; FULL_CLEANUP_STEPS.len()]);

        camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut context)
            .unwrap();

        assert_eq!(context.calls, FULL_CLEANUP_STEPS);
        assert!(context.results.is_empty());
        assert!(camera.handle.is_none());
        assert_eq!(camera.state, CameraState::Closed);

        // `Camera::close(self)` is followed by Drop. Both share this path, so
        // a second cleanup must not invoke any native function.
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
    fn cleanup_unregisters_uncertain_callback_registrations() {
        let mut image_camera = camera_with_handle(CameraState::Faulted);
        image_camera.retired_image_cbs.push(image_callback());
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
        exception_camera
            .retired_exception_cbs
            .push(exception_callback());
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
        event_camera
            .retired_event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        event_camera.uncertain_registration = Some(UncertainRegistration::Event(0));
        assert_cleanup_calls(
            event_camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );
    }

    #[test]
    fn uncertain_event_with_an_active_name_is_unregistered_only_once() {
        let mut camera = camera_with_handle(CameraState::Faulted);
        camera
            .event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        camera
            .retired_event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        camera.uncertain_registration = Some(UncertainRegistration::Event(0));

        assert_cleanup_calls(
            camera,
            &[
                CleanupStep::StopGrabbing,
                CleanupStep::UnregisterEventCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );
    }

    #[test]
    fn destroy_result_controls_whether_callback_tokens_are_dropped_or_leaked() {
        let successful_drops = Arc::new(AtomicUsize::new(0));
        let mut successful_camera = camera_with_probed_callbacks(&successful_drops);
        let mut success = FakeCleanup::with_results(vec![sys::MV_OK; 5]);
        successful_camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut success)
            .unwrap();
        assert_eq!(successful_drops.load(Ordering::SeqCst), 0);
        drop(successful_camera);
        assert_eq!(successful_drops.load(Ordering::SeqCst), 6);

        let leaked_drops = Arc::new(AtomicUsize::new(0));
        let mut failed_camera = camera_with_probed_callbacks(&leaked_drops);
        let mut failure = FakeCleanup::with_results([
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_E_RESOURCE,
        ]);
        let error = failed_camera
            .cleanup_with(&FAKE_CLEANUP_FNS, &mut failure)
            .unwrap_err();
        assert_eq!(error.failures().len(), 1);
        assert_eq!(error.failures()[0].step, CleanupStep::DestroyHandle);
        drop(failed_camera);
        assert_eq!(leaked_drops.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn successful_callback_unregisters_retire_all_three_token_kinds() {
        let mut camera = camera(CameraState::Open);
        camera.image_cb = Some(image_callback());
        camera.exception_cb = Some(exception_callback());
        camera
            .event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));

        camera.finish_image_unregister(Ok(())).unwrap();
        camera.finish_exception_unregister(Ok(())).unwrap();
        camera.finish_event_unregister(0, Ok(())).unwrap();

        assert!(camera.image_cb.is_none());
        assert!(camera.exception_cb.is_none());
        assert!(camera.event_cbs.is_empty());
        assert_eq!(camera.retired_image_cbs.len(), 1);
        assert_eq!(camera.retired_exception_cbs.len(), 1);
        assert_eq!(camera.retired_event_cbs.len(), 1);
        assert_eq!(camera.state, CameraState::Open);
    }

    #[test]
    fn failed_callback_unregisters_keep_tokens_active_and_fault_camera() {
        let mut image_camera = camera(CameraState::Open);
        image_camera.image_cb = Some(image_callback());
        assert!(matches!(
            image_camera.finish_image_unregister(Err(MvsError::Unknown(1))),
            Err(MvsError::Unknown(1))
        ));
        assert!(image_camera.image_cb.is_some());
        assert!(image_camera.retired_image_cbs.is_empty());
        assert_eq!(image_camera.state, CameraState::Faulted);

        let mut exception_camera = camera(CameraState::Grabbing);
        exception_camera.exception_cb = Some(exception_callback());
        assert!(matches!(
            exception_camera.finish_exception_unregister(Err(MvsError::Unknown(2))),
            Err(MvsError::Unknown(2))
        ));
        assert!(exception_camera.exception_cb.is_some());
        assert!(exception_camera.retired_exception_cbs.is_empty());
        assert_eq!(exception_camera.state, CameraState::Faulted);

        let mut event_camera = camera(CameraState::Open);
        event_camera
            .event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        assert!(matches!(
            event_camera.finish_event_unregister(0, Err(MvsError::Unknown(3))),
            Err(MvsError::Unknown(3))
        ));
        assert_eq!(event_camera.event_cbs.len(), 1);
        assert!(event_camera.retired_event_cbs.is_empty());
        assert_eq!(event_camera.state, CameraState::Faulted);
    }

    #[test]
    fn event_unregister_retires_only_the_named_registration() {
        let mut camera = camera(CameraState::Open);
        camera
            .event_cbs
            .push((CString::new("ExposureEnd").unwrap(), event_callback()));
        camera
            .event_cbs
            .push((CString::new("LineStart").unwrap(), event_callback()));

        camera.finish_event_unregister(0, Ok(())).unwrap();

        assert_eq!(camera.event_cbs.len(), 1);
        assert_eq!(camera.event_cbs[0].0.to_str().unwrap(), "LineStart");
        assert_eq!(camera.retired_event_cbs.len(), 1);
        assert_eq!(
            camera.retired_event_cbs[0].0.to_str().unwrap(),
            "ExposureEnd"
        );
    }

    #[test]
    fn faulted_and_closed_states_reject_normal_operations() {
        for state in [CameraState::Faulted, CameraState::Closed] {
            let mut camera = camera(state);
            camera.handle = Some(std::ptr::NonNull::<u8>::dangling().as_ptr().cast());

            assert!(matches!(camera.normal_handle(), Err(MvsError::CallOrder)));

            // Keep this synthetic handle away from any future teardown path.
            camera.handle = None;
        }
    }
}
