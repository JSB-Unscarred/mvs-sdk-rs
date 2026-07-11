//! Opened camera — the central type of the crate.
//!
//! A [`Camera`] owns an SDK handle and all registered closure-based callbacks.
//! Dropping it stops grabbing, closes the device, and destroys the handle
//! (in that order). Parameter access uses the SDK's native string-keyed API
//! verbatim: `cam.set_int("ExposureTime", 10000)?`.

use std::ffi::CString;
use std::os::raw::{c_float, c_void};
use std::sync::Mutex;

use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::error::check;
use crate::sys;
use crate::{AccessMode, EnumNode, FloatNode, IntNode, MvsResult};

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

/// An opened MVS camera. `Send` but not `Sync`: the SDK serializes internally,
/// but concurrent calls on the same handle still require external
/// synchronization.
pub(crate) struct Camera {
    handle: *mut c_void,
    grabbing: bool,
    image_cb: Option<CallbackRegistration<ImageCallback>>,
    retired_image_cbs: Vec<CallbackRegistration<ImageCallback>>,
    exception_cb: Option<CallbackRegistration<ExceptionCallback>>,
    retired_exception_cbs: Vec<CallbackRegistration<ExceptionCallback>>,
    event_cbs: Vec<(CString, CallbackRegistration<EventCallback>)>,
    retired_event_cbs: Vec<(CString, CallbackRegistration<EventCallback>)>,
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
            handle,
            grabbing: false,
            image_cb: None,
            retired_image_cbs: Vec::new(),
            exception_cb: None,
            retired_exception_cbs: Vec::new(),
            event_cbs: Vec::new(),
            retired_event_cbs: Vec::new(),
        })
    }

    /// Raw handle, for advanced use-cases.
    pub(crate) fn as_raw_handle(&self) -> *mut c_void {
        self.handle
    }

    pub(crate) fn is_connected(&self) -> bool {
        // SAFETY: handle was validated at open().
        unsafe { sys::MV_CC_IsDeviceConnected(self.handle) != 0 }
    }

    // ---- Grabbing control ----

    pub(crate) fn start_grabbing(&mut self) -> MvsResult<()> {
        // SAFETY: handle valid.
        let code = unsafe { sys::MV_CC_StartGrabbing(self.handle) };
        check(code)?;
        self.grabbing = true;
        Ok(())
    }

    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        // SAFETY: handle valid.
        let code = unsafe { sys::MV_CC_StopGrabbing(self.handle) };
        check(code)?;
        self.grabbing = false;
        Ok(())
    }

    /// Poll for an image, waiting up to `timeout_ms` milliseconds. The
    /// returned [`FrameGuard`] releases the SDK buffer on drop.
    pub(crate) fn get_image_buffer(&mut self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw is zero-initialized and will be populated by the SDK.
        let code = unsafe { sys::MV_CC_GetImageBuffer(self.handle, &mut raw, timeout_ms) };
        check(code)?;
        Ok(FrameGuard::new(self.handle, raw))
    }

    // ---- Callback registration ----

    /// Register an image callback. The closure runs on the SDK's streaming
    /// thread; keep it short or forward the frame through a channel.
    ///
    /// Replacing the callback while grabbing is active is technically
    /// supported by the SDK, but to be safe call [`Camera::stop_grabbing`]
    /// first.
    pub(crate) fn register_image_callback(&mut self, f: ImageCallbackFn) -> MvsResult<()> {
        // Ensure every post-FFI move into retired storage is allocation-free.
        self.retired_image_cbs.reserve(1);
        let cb = CallbackRegistration::new(ImageCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: the trampoline has the ABI expected by the SDK, and `user`
        // is a stable Arc token retained by this backend.
        let code = unsafe {
            sys::MV_CC_RegisterImageCallBackEx(self.handle, Some(image_trampoline), user)
        };
        if let Err(error) = check(code) {
            // Retain the token until handle destruction even on failure, in
            // case the native API stored pUser before reporting the error.
            self.retired_image_cbs.push(cb);
            return Err(error);
        }
        if let Some(previous) = self.image_cb.replace(cb) {
            self.retired_image_cbs.push(previous);
        }
        Ok(())
    }

    /// Unregister the image callback (passes `NULL` to the SDK).
    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        if self.image_cb.is_some() {
            self.retired_image_cbs.reserve(1);
        }
        // SAFETY: handle valid; passing None + null user to deregister.
        let code =
            unsafe { sys::MV_CC_RegisterImageCallBackEx(self.handle, None, std::ptr::null_mut()) };
        check(code)?;
        if let Some(previous) = self.image_cb.take() {
            self.retired_image_cbs.push(previous);
        }
        Ok(())
    }

    /// Register an exception callback. Invoked by the SDK on device-level
    /// errors (disconnect, etc.). The argument is the SDK's raw message type.
    pub(crate) fn register_exception_callback(&mut self, f: ExceptionCallbackFn) -> MvsResult<()> {
        self.retired_exception_cbs.reserve(1);
        let cb = CallbackRegistration::new(ExceptionCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: see register_image_callback.
        let code = unsafe {
            sys::MV_CC_RegisterExceptionCallBack(self.handle, Some(exception_trampoline), user)
        };
        if let Err(error) = check(code) {
            self.retired_exception_cbs.push(cb);
            return Err(error);
        }
        if let Some(previous) = self.exception_cb.replace(cb) {
            self.retired_exception_cbs.push(previous);
        }
        Ok(())
    }

    /// Register an event callback for the named GenICam event (e.g. a custom
    /// trigger or line-state change). Multiple events can be registered; they
    /// are stored independently.
    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        f: EventCallbackFn,
    ) -> MvsResult<()> {
        let name = CString::new(event_name)?;
        self.event_cbs.reserve(1);
        self.retired_event_cbs.reserve(1);
        let cb = CallbackRegistration::new(EventCallback(Mutex::new(f)));
        let user = cb.user_data();
        // SAFETY: `name` remains alive while registered, and `user` points to
        // a stable Arc token retained by this backend.
        let code = unsafe {
            sys::MV_CC_RegisterEventCallBackEx(
                self.handle,
                name.as_ptr(),
                Some(event_trampoline),
                user,
            )
        };
        if let Err(error) = check(code) {
            self.retired_event_cbs.push((name, cb));
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

    /// Enable SDK event notification for the named GenICam event.
    pub(crate) fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        let name = CString::new(event_name)?;
        // SAFETY: handle is valid and name lives for the duration of the call.
        let code = unsafe { sys::MV_CC_EventNotificationOn(self.handle, name.as_ptr()) };
        check(code)
    }

    /// Disable SDK event notification for the named GenICam event.
    pub(crate) fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        let name = CString::new(event_name)?;
        // SAFETY: handle is valid and name lives for the duration of the call.
        let code = unsafe { sys::MV_CC_EventNotificationOff(self.handle, name.as_ptr()) };
        check(code)
    }

    // ---- Parameter access (SDK string-key style) ----

    /// Set an integer node (`MV_CC_SetIntValueEx`). Typical keys: `"Width"`,
    /// `"Height"`, `"OffsetX"`.
    pub(crate) fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: key points at `k` for the duration of the call.
        let code = unsafe { sys::MV_CC_SetIntValueEx(self.handle, k.as_ptr(), value) };
        check(code)
    }

    /// Read an integer node (`MV_CC_GetIntValueEx`). Returns the node's
    /// current value; use [`Camera::get_int_range`] if you also need min/max.
    pub(crate) fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.get_int_range(key).map(|v| v.current)
    }

    /// Read an integer node with its full range information.
    pub(crate) fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        let k = CString::new(key)?;
        let mut value = sys::MVCC_INTVALUE_EX::default();
        // SAFETY: value is stack-allocated; key valid for call.
        let code = unsafe { sys::MV_CC_GetIntValueEx(self.handle, k.as_ptr(), &mut value) };
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
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetFloatValue(self.handle, k.as_ptr(), value as c_float) };
        check(code)
    }

    pub(crate) fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.get_float_range(key).map(|v| v.current)
    }

    /// Read a float node with its full range information.
    pub(crate) fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        let k = CString::new(key)?;
        let mut value = sys::MVCC_FLOATVALUE::default();
        // SAFETY: see get_int_range.
        let code = unsafe { sys::MV_CC_GetFloatValue(self.handle, k.as_ptr(), &mut value) };
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
        let k = CString::new(key)?;
        // The SDK typedef's C `bool` as `char`; pass 0/1 as i8.
        let v: sys::bool_ = if value { 1 } else { 0 };
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetBoolValue(self.handle, k.as_ptr(), v) };
        check(code)
    }

    pub(crate) fn get_bool(&self, key: &str) -> MvsResult<bool> {
        let k = CString::new(key)?;
        let mut out: sys::bool_ = 0;
        // SAFETY: see get_int.
        let code = unsafe { sys::MV_CC_GetBoolValue(self.handle, k.as_ptr(), &mut out) };
        check(code)?;
        Ok(out != 0)
    }

    /// Set an enum node by symbolic name (`MV_CC_SetEnumValueByString`).
    /// Example: `cam.set_enum("TriggerMode", "On")`.
    pub(crate) fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: both strings live for the duration of the call.
        let code = unsafe { sys::MV_CC_SetEnumValueByString(self.handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Set a string node (`MV_CC_SetStringValue`), e.g. `"DeviceUserID"`.
    pub(crate) fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: see set_enum.
        let code = unsafe { sys::MV_CC_SetStringValue(self.handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Execute a command node (`MV_CC_SetCommandValue`), e.g.
    /// `cam.exec_command("TriggerSoftware")`.
    pub(crate) fn exec_command(&self, key: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetCommandValue(self.handle, k.as_ptr()) };
        check(code)
    }

    /// Read a string node (`MV_CC_GetStringValue`). Returns up to 255 bytes.
    pub(crate) fn get_string(&self, key: &str) -> MvsResult<String> {
        let k = CString::new(key)?;
        let mut value = sys::MVCC_STRINGVALUE::default();
        // SAFETY: value is stack-allocated; key valid for call.
        let code = unsafe { sys::MV_CC_GetStringValue(self.handle, k.as_ptr(), &mut value) };
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
        let k = CString::new(key)?;
        let mut value = sys::MVCC_ENUMVALUE::default();
        // SAFETY: see get_string.
        let code = unsafe { sys::MV_CC_GetEnumValue(self.handle, k.as_ptr(), &mut value) };
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
        let k = CString::new(key)?;
        // SAFETY: see exec_command.
        let code = unsafe { sys::MV_CC_SetEnumValue(self.handle, k.as_ptr(), value) };
        check(code)
    }
}

impl Camera {
    pub(crate) fn debug_details(&self) -> (bool, bool, bool, usize) {
        (
            self.grabbing,
            self.image_cb.is_some(),
            self.exception_cb.is_some(),
            self.event_cbs.len(),
        )
    }

    pub(crate) fn close(&mut self) {
        if self.handle.is_null() {
            return;
        }

        // Tear down in reverse of open(). A successful DestroyHandle is the
        // SDK's quiescence boundary: after it returns, no new callback may use
        // one of the registered user pointers.
        let destroyed = unsafe {
            if self.grabbing {
                let _ = sys::MV_CC_StopGrabbing(self.handle);
            }
            if self.image_cb.is_some() {
                let _ = sys::MV_CC_RegisterImageCallBackEx(self.handle, None, std::ptr::null_mut());
            }
            if self.exception_cb.is_some() {
                let _ =
                    sys::MV_CC_RegisterExceptionCallBack(self.handle, None, std::ptr::null_mut());
            }
            for (name, _) in &self.event_cbs {
                let _ = sys::MV_CC_RegisterEventCallBackEx(
                    self.handle,
                    name.as_ptr(),
                    None,
                    std::ptr::null_mut(),
                );
            }
            let _ = sys::MV_CC_CloseDevice(self.handle);
            sys::MV_CC_DestroyHandle(self.handle) as u32 == sys::MV_OK
        };

        if !destroyed {
            // DestroyHandle failed, so the native side may still retain one
            // or more callback pointers. Leaking their strong references is
            // safer than freeing memory the SDK could call into later.
            self.leak_callbacks();
        }

        // On failure the native handle is intentionally leaked as well; this
        // Rust wrapper is being dropped and must not attempt teardown again.
        self.handle = std::ptr::null_mut();
        self.grabbing = false;
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
