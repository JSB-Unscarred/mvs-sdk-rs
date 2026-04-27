//! Opened camera — the central type of the crate.
//!
//! A [`Camera`] owns an SDK handle and all registered closure-based callbacks.
//! Dropping it stops grabbing, closes the device, and destroys the handle
//! (in that order). Parameter access uses the SDK's native string-keyed API
//! verbatim: `cam.set_int("ExposureTime", 10000)?`.

use std::ffi::CString;
use std::fmt;
use std::os::raw::{c_float, c_void};
use std::sync::Arc;

use crate::MvsResult;
use crate::callback::{
    EventCallback, EventInfo, ExceptionCallback, ImageCallback, event_trampoline,
    exception_trampoline, image_trampoline,
};
use crate::error::check;
use crate::frame::{Frame, FrameGuard};
use crate::library::Sdk;
use crate::sys;

// ---------------------------------------------------------------------------
// AccessMode
// ---------------------------------------------------------------------------

/// Device access mode passed to [`Camera::open`] / [`DeviceInfo::open`].
///
/// [`DeviceInfo::open`]: crate::DeviceInfo::open
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AccessMode {
    Exclusive,
    ExclusiveWithSwitch,
    Control,
    ControlWithSwitch,
    ControlSwitchEnable,
    ControlSwitchEnableWithKey,
    Monitor,
}

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
// Node value types (returned by the range/info getters)
// ---------------------------------------------------------------------------

/// Full integer-node information: current value plus its allowed range.
#[derive(Copy, Clone, Debug)]
pub struct IntNode {
    pub current: i64,
    pub min: i64,
    pub max: i64,
    pub inc: i64,
}

/// Full float-node information: current value plus min/max.
#[derive(Copy, Clone, Debug)]
pub struct FloatNode {
    pub current: f32,
    pub min: f32,
    pub max: f32,
}

/// Enum-node information: current numeric value and the list of allowed
/// values (numeric — use [`Camera::set_enum`] with symbolic names).
#[derive(Clone, Debug)]
pub struct EnumNode {
    pub current: u32,
    pub supported: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// An opened MVS camera. `Send` but not `Sync`: the SDK serializes internally,
/// but concurrent calls on the same handle still require external
/// synchronization.
pub struct Camera {
    handle: *mut c_void,
    _library: Arc<Sdk>,
    grabbing: bool,
    image_cb: Option<Box<ImageCallback>>,
    exception_cb: Option<Box<ExceptionCallback>>,
    event_cbs: Vec<(CString, Box<EventCallback>)>,
}

// SAFETY: the handle is usable from any thread; we just don't allow concurrent
// calls on the same Camera (hence !Sync).
unsafe impl Send for Camera {}

impl Camera {
    pub(crate) fn open(
        library: &Arc<Sdk>,
        dev_info: &sys::MV_CC_DEVICE_INFO,
        mode: AccessMode,
    ) -> MvsResult<Self> {
        let mut handle: *mut c_void = std::ptr::null_mut();

        // SAFETY: handle is owned locally until success; dev_info remains valid
        // for the duration of this call (borrowed from DeviceList).
        let code = unsafe { sys::MV_CC_CreateHandle(&mut handle, dev_info) };
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
            _library: Arc::clone(library),
            grabbing: false,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
        })
    }

    /// Raw handle, for advanced use-cases.
    pub fn as_raw_handle(&self) -> *mut c_void {
        self.handle
    }

    pub fn is_connected(&self) -> bool {
        // SAFETY: handle was validated at open().
        unsafe { sys::MV_CC_IsDeviceConnected(self.handle) != 0 }
    }

    // ---- Grabbing control ----

    pub fn start_grabbing(&mut self) -> MvsResult<()> {
        // SAFETY: handle valid.
        let code = unsafe { sys::MV_CC_StartGrabbing(self.handle) };
        check(code)?;
        self.grabbing = true;
        Ok(())
    }

    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        // SAFETY: handle valid.
        let code = unsafe { sys::MV_CC_StopGrabbing(self.handle) };
        check(code)?;
        self.grabbing = false;
        Ok(())
    }

    /// Poll for an image, waiting up to `timeout_ms` milliseconds. The
    /// returned [`FrameGuard`] releases the SDK buffer on drop.
    pub fn get_image_buffer(&mut self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
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
    pub fn register_image_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(&Frame<'_>) + Send + 'static,
    {
        let mut cb = Box::new(ImageCallback(Box::new(f)));
        let user = cb.as_mut() as *mut ImageCallback as *mut c_void;
        // SAFETY: trampoline is extern "system" fn; user points at pinned box.
        let code = unsafe {
            sys::MV_CC_RegisterImageCallBackEx(self.handle, Some(image_trampoline), user)
        };
        check(code)?;
        self.image_cb = Some(cb);
        Ok(())
    }

    /// Unregister the image callback (passes `NULL` to the SDK).
    pub fn unregister_image_callback(&mut self) -> MvsResult<()> {
        // SAFETY: handle valid; passing None + null user to deregister.
        let code =
            unsafe { sys::MV_CC_RegisterImageCallBackEx(self.handle, None, std::ptr::null_mut()) };
        check(code)?;
        // Drop the box after the SDK has accepted the new registration.
        self.image_cb = None;
        Ok(())
    }

    /// Register an exception callback. Invoked by the SDK on device-level
    /// errors (disconnect, etc.). The argument is the SDK's raw message type.
    pub fn register_exception_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(u32) + Send + 'static,
    {
        let mut cb = Box::new(ExceptionCallback(Box::new(f)));
        let user = cb.as_mut() as *mut ExceptionCallback as *mut c_void;
        // SAFETY: see register_image_callback.
        let code = unsafe {
            sys::MV_CC_RegisterExceptionCallBack(self.handle, Some(exception_trampoline), user)
        };
        check(code)?;
        self.exception_cb = Some(cb);
        Ok(())
    }

    /// Register an event callback for the named GenICam event (e.g. a custom
    /// trigger or line-state change). Multiple events can be registered; they
    /// are stored independently.
    pub fn register_event_callback<F>(&mut self, event_name: &str, f: F) -> MvsResult<()>
    where
        F: FnMut(&EventInfo<'_>) + Send + 'static,
    {
        let name = CString::new(event_name)?;
        let mut cb = Box::new(EventCallback(Box::new(f)));
        let user = cb.as_mut() as *mut EventCallback as *mut c_void;
        // SAFETY: name.as_ptr() is valid for the call; cb is pinned in self.
        let code = unsafe {
            sys::MV_CC_RegisterEventCallBackEx(
                self.handle,
                name.as_ptr(),
                Some(event_trampoline),
                user,
            )
        };
        check(code)?;
        // Remove any previous registration under the same name, then store.
        self.event_cbs
            .retain(|(n, _)| n.as_c_str() != name.as_c_str());
        self.event_cbs.push((name, cb));
        Ok(())
    }

    // ---- Parameter access (SDK string-key style) ----

    /// Set an integer node (`MV_CC_SetIntValueEx`). Typical keys: `"Width"`,
    /// `"Height"`, `"OffsetX"`.
    pub fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: key points at `k` for the duration of the call.
        let code = unsafe { sys::MV_CC_SetIntValueEx(self.handle, k.as_ptr(), value) };
        check(code)
    }

    /// Read an integer node (`MV_CC_GetIntValueEx`). Returns the node's
    /// current value; use [`Camera::get_int_range`] if you also need min/max.
    pub fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.get_int_range(key).map(|v| v.current)
    }

    /// Read an integer node with its full range information.
    pub fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
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
    pub fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetFloatValue(self.handle, k.as_ptr(), value as c_float) };
        check(code)
    }

    pub fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.get_float_range(key).map(|v| v.current)
    }

    /// Read a float node with its full range information.
    pub fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
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
    pub fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        let k = CString::new(key)?;
        // The SDK typedef's C `bool` as `char`; pass 0/1 as i8.
        let v: sys::bool_ = if value { 1 } else { 0 };
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetBoolValue(self.handle, k.as_ptr(), v) };
        check(code)
    }

    pub fn get_bool(&self, key: &str) -> MvsResult<bool> {
        let k = CString::new(key)?;
        let mut out: sys::bool_ = 0;
        // SAFETY: see get_int.
        let code = unsafe { sys::MV_CC_GetBoolValue(self.handle, k.as_ptr(), &mut out) };
        check(code)?;
        Ok(out != 0)
    }

    /// Set an enum node by symbolic name (`MV_CC_SetEnumValueByString`).
    /// Example: `cam.set_enum("TriggerMode", "On")`.
    pub fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: both strings live for the duration of the call.
        let code = unsafe { sys::MV_CC_SetEnumValueByString(self.handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Set a string node (`MV_CC_SetStringValue`), e.g. `"DeviceUserID"`.
    pub fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        let v = CString::new(value)?;
        // SAFETY: see set_enum.
        let code = unsafe { sys::MV_CC_SetStringValue(self.handle, k.as_ptr(), v.as_ptr()) };
        check(code)
    }

    /// Execute a command node (`MV_CC_SetCommandValue`), e.g.
    /// `cam.exec_command("TriggerSoftware")`.
    pub fn exec_command(&self, key: &str) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: see set_int.
        let code = unsafe { sys::MV_CC_SetCommandValue(self.handle, k.as_ptr()) };
        check(code)
    }

    /// Read a string node (`MV_CC_GetStringValue`). Returns up to 255 bytes.
    pub fn get_string(&self, key: &str) -> MvsResult<String> {
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
    pub fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.get_enum_info(key).map(|v| v.current)
    }

    /// Read an enum node with its supported-values list
    /// (`MV_CC_GetEnumValue`, up to 64 entries — use the SDK's `Ex` variant
    /// yourself for the 256-entry form).
    pub fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
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
    pub fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        let k = CString::new(key)?;
        // SAFETY: see exec_command.
        let code = unsafe { sys::MV_CC_SetEnumValue(self.handle, k.as_ptr(), value) };
        check(code)
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera")
            .field("handle", &self.handle)
            .field("grabbing", &self.grabbing)
            .field("image_cb", &self.image_cb.is_some())
            .field("exception_cb", &self.exception_cb.is_some())
            .field("event_cbs", &self.event_cbs.len())
            .finish()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // Tear down in reverse of open(): stop grabbing, deregister callbacks,
        // close device, destroy handle. Ignore errors — nothing to do from
        // Drop.
        unsafe {
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
            for (name, callback) in self.event_cbs.drain(..) {
                let _ = sys::MV_CC_RegisterEventCallBackEx(
                    self.handle,
                    name.as_ptr(),
                    None,
                    std::ptr::null_mut(),
                );
                drop(callback);
            }
            let _ = sys::MV_CC_CloseDevice(self.handle);
            let _ = sys::MV_CC_DestroyHandle(self.handle);
        }
    }
}
