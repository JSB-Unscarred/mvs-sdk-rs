//! Opened camera — the central type of the crate.
//!
//! A [`Camera`] owns an SDK handle and all registered closure-based callbacks.
//! Dropping it stops grabbing, closes the device, and destroys the handle
//! (in that order). Parameter access uses the SDK's native string-keyed API
//! verbatim: `cam.set_int("ExposureTime", 10000)?`.

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::sync::Arc;

use crate::backend::AcquisitionState;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationState {
    Unregistered,
    Registered,
    Unknown,
}

impl RegistrationState {
    fn needs_unregister(self) -> bool {
        !matches!(self, Self::Unregistered)
    }
}

struct CallbackRecord<C> {
    slot: Arc<CallbackSlot<C>>,
    native_slot: *const CallbackSlot<C>,
    registration: RegistrationState,
}

impl<C> CallbackRecord<C> {
    fn new() -> Self {
        // Preserve a pointer produced by Arc::into_raw, as required by
        // Arc::increment_strong_count in the trampoline, while immediately
        // reconstructing the camera-owned strong reference. The record keeps
        // that Arc alive for as long as the SDK may retain pUser.
        let native_slot = Arc::into_raw(Arc::new(CallbackSlot::new()));
        // SAFETY: `native_slot` was just produced by Arc::into_raw and is
        // reconstructed exactly once. Later trampoline increments create
        // independent strong references before calling Arc::from_raw.
        let slot = unsafe { Arc::from_raw(native_slot) };
        Self {
            slot,
            native_slot,
            registration: RegistrationState::Unregistered,
        }
    }

    fn user_data(&self) -> *mut c_void {
        self.native_slot.cast_mut().cast()
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

struct AcquisitionFns<C> {
    start_grabbing: fn(&mut C, *mut c_void) -> c_int,
    stop_grabbing: fn(&mut C, *mut c_void) -> c_int,
}

impl AcquisitionFns<()> {
    const NATIVE: Self = Self {
        start_grabbing: native_start_grabbing,
        stop_grabbing: native_stop_grabbing,
    };
}

fn native_start_grabbing(_: &mut (), handle: *mut c_void) -> c_int {
    // SAFETY: the caller supplies a live camera handle.
    unsafe { sys::MV_CC_StartGrabbing(handle) }
}

fn native_stop_grabbing(_: &mut (), handle: *mut c_void) -> c_int {
    // SAFETY: the caller supplies a live camera handle.
    unsafe { sys::MV_CC_StopGrabbing(handle) }
}

struct OpenFns<C> {
    create_handle: fn(&mut C, *mut *mut c_void, *const sys::MV_CC_DEVICE_INFO) -> c_int,
    open_device: fn(&mut C, *mut c_void, u32, u16) -> c_int,
    destroy_handle: fn(&mut C, *mut c_void) -> c_int,
}

impl OpenFns<()> {
    const NATIVE: Self = Self {
        create_handle: native_create_handle,
        open_device: native_open_device,
        destroy_handle: native_open_rollback_destroy_handle,
    };
}

fn native_create_handle(
    _: &mut (),
    handle: *mut *mut c_void,
    device: *const sys::MV_CC_DEVICE_INFO,
) -> c_int {
    // SAFETY: the caller supplies writable handle storage and a live copied
    // device-info record.
    unsafe { sys::MV_CC_CreateHandle(handle, device) }
}

fn native_open_device(
    _: &mut (),
    handle: *mut c_void,
    access_mode: u32,
    switchover_key: u16,
) -> c_int {
    // SAFETY: the caller supplies a handle returned by CreateHandle.
    unsafe { sys::MV_CC_OpenDevice(handle, access_mode, switchover_key) }
}

fn native_open_rollback_destroy_handle(_: &mut (), handle: *mut c_void) -> c_int {
    // SAFETY: the caller supplies a handle returned by CreateHandle whose
    // OpenDevice operation failed.
    unsafe { sys::MV_CC_DestroyHandle(handle) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandleDisposition {
    /// `MV_CC_DestroyHandle` returned `MV_OK`.
    Destroyed,
    /// Destruction was skipped or did not return `MV_OK`; native ownership is
    /// therefore unresolved.
    Orphaned,
}

#[derive(Debug)]
pub(crate) struct OpenFailure {
    pub(crate) error: MvsError,
    pub(crate) rollback_error: Option<MvsError>,
    /// `None` means CreateHandle itself failed and no native handle existed.
    pub(crate) disposition: Option<HandleDisposition>,
}

#[derive(Debug)]
#[must_use = "cleanup reports must settle the camera's native-handle lease"]
pub(crate) struct CleanupReport {
    pub(crate) result: Result<(), CleanupError>,
    /// `None` means this Camera had already consumed its handle, so a caller
    /// must not settle its resource lease a second time.
    pub(crate) disposition: Option<HandleDisposition>,
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

fn int_node_from_raw(value: &sys::MVCC_INTVALUE_EX) -> IntNode {
    IntNode {
        current: value.nCurValue,
        min: value.nMin,
        max: value.nMax,
        inc: value.nInc,
    }
}

fn float_node_from_raw(value: &sys::MVCC_FLOATVALUE) -> FloatNode {
    FloatNode {
        current: value.fCurValue,
        min: value.fMin,
        max: value.fMax,
    }
}

fn bool_from_raw(value: sys::bool_) -> bool {
    value != 0
}

fn string_from_raw(value: &sys::MVCC_STRINGVALUE) -> String {
    let bytes = &value.chCurValue;
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    // SAFETY: c_char is i8 on Windows; reinterpret bytes as u8 for UTF-8.
    let slice = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), end) };
    String::from_utf8_lossy(slice).into_owned()
}

fn enum_node_from_raw(value: &sys::MVCC_ENUMVALUE) -> EnumNode {
    let supported_len = (value.nSupportedNum as usize).min(value.nSupportValue.len());
    EnumNode {
        current: value.nCurValue,
        supported: value.nSupportValue[..supported_len].to_vec(),
    }
}

/// An opened MVS camera. `Send` but not `Sync`: the SDK serializes internally,
/// but concurrent calls on the same handle still require external
/// synchronization.
pub(crate) struct Camera {
    handle: Option<*mut c_void>,
    acquisition: AcquisitionState,
    image_cb: Option<CallbackRecord<ImageCallbackFn>>,
    exception_cb: Option<CallbackRecord<ExceptionCallbackFn>>,
    event_cbs: Vec<EventRecord>,
}

// SAFETY: the handle is usable from any thread; we just don't allow concurrent
// calls on the same Camera (hence !Sync).
impl Camera {
    pub(crate) fn open(device: DeviceInfo, mode: AccessMode) -> Result<Self, OpenFailure> {
        Self::open_with(device.raw(), mode, &OpenFns::NATIVE, &mut ())
    }

    fn open_with<C>(
        device: &sys::MV_CC_DEVICE_INFO,
        mode: AccessMode,
        fns: &OpenFns<C>,
        context: &mut C,
    ) -> Result<Self, OpenFailure> {
        let mut handle: *mut c_void = std::ptr::null_mut();

        let code = (fns.create_handle)(context, &mut handle, device);
        if let Err(error) = check(code) {
            return Err(OpenFailure {
                error,
                rollback_error: None,
                disposition: None,
            });
        }

        let code = (fns.open_device)(context, handle, mode.raw(), 0);
        if let Err(error) = check(code) {
            let rollback = check((fns.destroy_handle)(context, handle));
            let (rollback_error, disposition) = match rollback {
                Ok(()) => (None, HandleDisposition::Destroyed),
                Err(rollback_error) => (Some(rollback_error), HandleDisposition::Orphaned),
            };
            return Err(OpenFailure {
                error,
                rollback_error,
                disposition: Some(disposition),
            });
        }

        Ok(Self {
            handle: Some(handle),
            acquisition: AcquisitionState::Stopped,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
        })
    }

    fn normal_handle(&self) -> MvsResult<*mut c_void> {
        self.handle.ok_or(MvsError::CallOrder)
    }

    fn is_exception_callback_context(&self) -> bool {
        self.exception_cb
            .as_ref()
            .is_some_and(|record| record.slot.is_current())
    }

    fn is_restricted_callback_context(&self) -> bool {
        self.image_cb
            .as_ref()
            .is_some_and(|record| record.slot.is_current())
            || self
                .event_cbs
                .iter()
                .any(|record| record.callback.slot.is_current())
    }

    fn is_callback_context(&self) -> bool {
        self.is_restricted_callback_context() || self.is_exception_callback_context()
    }

    fn reject_callback_context(&self) -> MvsResult<()> {
        if self.is_callback_context() {
            Err(MvsError::CallOrder)
        } else {
            Ok(())
        }
    }

    fn handle_in_acquisition(&self, expected: AcquisitionState) -> MvsResult<*mut c_void> {
        if self.acquisition != expected {
            return Err(MvsError::CallOrder);
        }
        self.normal_handle()
    }

    fn polling_handle(&self) -> MvsResult<*mut c_void> {
        self.handle_in_acquisition(AcquisitionState::Polling)
    }

    fn begin_grabbing(&self) -> MvsResult<(*mut c_void, AcquisitionState)> {
        self.reject_callback_context()?;
        let handle = self.handle_in_acquisition(AcquisitionState::Stopped)?;
        let target = match self.image_cb.as_ref().map(|record| record.registration) {
            Some(RegistrationState::Registered) => AcquisitionState::Callback,
            Some(RegistrationState::Unknown) => return Err(MvsError::CallOrder),
            Some(RegistrationState::Unregistered) | None => AcquisitionState::Polling,
        };
        Ok((handle, target))
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
        self.start_grabbing_with(&AcquisitionFns::NATIVE, &mut ())
    }

    fn start_grabbing_with<C>(
        &mut self,
        fns: &AcquisitionFns<C>,
        context: &mut C,
    ) -> MvsResult<()> {
        let (handle, target) = self.begin_grabbing()?;
        let result = check((fns.start_grabbing)(context, handle));
        self.acquisition = if result.is_ok() {
            target
        } else {
            AcquisitionState::Unknown
        };
        result
    }

    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.stop_grabbing_with(&AcquisitionFns::NATIVE, &mut ())
    }

    fn stop_grabbing_with<C>(&mut self, fns: &AcquisitionFns<C>, context: &mut C) -> MvsResult<()> {
        self.reject_callback_context()?;
        if !self.acquisition.requires_stop() {
            return Err(MvsError::CallOrder);
        }
        let handle = self.normal_handle()?;
        let result = check((fns.stop_grabbing)(context, handle));
        self.acquisition = if result.is_ok() {
            AcquisitionState::Stopped
        } else {
            AcquisitionState::Unknown
        };
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
        let handle = self.handle_in_acquisition(AcquisitionState::Stopped)?;

        let record = self.image_cb.get_or_insert_with(CallbackRecord::new);
        if record.registration == RegistrationState::Unknown {
            return Err(MvsError::CallOrder);
        }
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.registration == RegistrationState::Registered {
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
            self.image_cb
                .as_mut()
                .expect("image callback record inserted above")
                .registration = RegistrationState::Unknown;
            return Err(error);
        }
        self.image_cb
            .as_mut()
            .expect("image callback record inserted above")
            .registration = RegistrationState::Registered;
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
        let handle = self.handle_in_acquisition(AcquisitionState::Stopped)?;
        let Some(record) = self.image_cb.as_ref() else {
            return Ok(());
        };
        if record.registration == RegistrationState::Unregistered {
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
                .registration = RegistrationState::Unregistered;
        } else {
            self.image_cb
                .as_mut()
                .expect("image callback record checked above")
                .registration = RegistrationState::Unknown;
        }
        result
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
        if record.registration == RegistrationState::Unknown {
            return Err(MvsError::CallOrder);
        }
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.registration == RegistrationState::Registered {
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
            self.exception_cb
                .as_mut()
                .expect("exception callback record inserted above")
                .registration = RegistrationState::Unknown;
            return Err(error);
        }
        self.exception_cb
            .as_mut()
            .expect("exception callback record inserted above")
            .registration = RegistrationState::Registered;
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
        if record.registration == RegistrationState::Unregistered {
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
                .registration = RegistrationState::Unregistered;
        } else {
            self.exception_cb
                .as_mut()
                .expect("exception callback record checked above")
                .registration = RegistrationState::Unknown;
        }
        result
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
        if record.registration == RegistrationState::Unknown {
            return Err(MvsError::CallOrder);
        }
        let previous = record.slot.activate(f);
        if let Some(previous) = previous {
            drop_callback_safely(previous);
        }
        if record.registration == RegistrationState::Registered {
            return Ok(());
        }

        let name_ptr = self.event_cbs[index].name.as_ptr();
        let user = record.user_data();
        let code = (fns.event)(context, handle, name_ptr, Some(event_trampoline), user);
        if let Err(error) = check(code) {
            if let Some(callback) = self.event_cbs[index].callback.slot.deactivate() {
                drop_callback_safely(callback);
            }
            self.event_cbs[index].callback.registration = RegistrationState::Unknown;
            return Err(error);
        }
        self.event_cbs[index].callback.registration = RegistrationState::Registered;
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
        if self.event_cbs[index].callback.registration == RegistrationState::Unregistered {
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
            self.event_cbs[index].callback.registration = RegistrationState::Unregistered;
        } else {
            self.event_cbs[index].callback.registration = RegistrationState::Unknown;
        }
        result
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
        Ok(int_node_from_raw(&value))
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
        Ok(float_node_from_raw(&value))
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
        Ok(bool_from_raw(out))
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
        Ok(string_from_raw(&value))
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
        Ok(enum_node_from_raw(&value))
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
        let (state, acquisition_mode) = if self.handle.is_none() {
            ("Closed", None)
        } else {
            match self.acquisition {
                AcquisitionState::Stopped => ("Open", None),
                AcquisitionState::Callback | AcquisitionState::Polling => {
                    ("Grabbing", self.acquisition.mode_name())
                }
                AcquisitionState::Unknown => ("Open", Some(self.acquisition.name())),
            }
        };
        (
            state,
            acquisition_mode,
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

    pub(crate) fn cleanup(&mut self) -> CleanupReport {
        self.cleanup_with(&CleanupFns::NATIVE, &mut ())
    }

    fn cleanup_with<C>(&mut self, fns: &CleanupFns<C>, context: &mut C) -> CleanupReport {
        if self.handle.is_none() {
            return CleanupReport {
                result: Ok(()),
                disposition: None,
            };
        }

        // The SDK's reconnect sample explicitly closes and destroys a handle
        // from its device-disconnect exception callback. It gives no such
        // guarantee for image callbacks (whose frame is still borrowed) or
        // event callbacks, so those contexts retain the conservative path.
        if self.is_restricted_callback_context() {
            return self.abandon_from_restricted_callback_context();
        }
        let from_exception_callback = self.is_exception_callback_context();

        // Revoke admission for every slot before waiting for any one slot.
        // This prevents a busy callback from allowing another callback to
        // enter while cleanup is already under way.
        self.stop_accepting_callbacks();
        if from_exception_callback {
            // The current exception trampoline owns the closure mutex and a
            // temporary Arc reference. Drain every other slot now; it will
            // remove this closure and release the last Arc after it returns.
            self.drain_callbacks_except_current_exception();
        } else {
            self.drain_callbacks();
        }

        // Reserve before consuming the handle so recording an error cannot
        // allocate between native calls. Normal cleanup has at most five
        // non-event attempts, one uncertain event, and one attempt per active
        // event; exception-context cleanup uses only two of these slots.
        let mut failures = Vec::with_capacity(self.event_cbs.len().saturating_add(6));
        let handle = self.handle.take().expect("handle checked above");

        // Normal cleanup tears down in reverse of open(). Inside an exception
        // callback, stay within the SDK's documented reconnect pattern:
        // CloseDevice followed by DestroyHandle, without stopping acquisition
        // or unregistering the callback that is currently executing.
        if !from_exception_callback {
            if self.acquisition.requires_stop() {
                let code = (fns.stop_grabbing)(context, handle);
                record_cleanup_result(&mut failures, CleanupStep::StopGrabbing, code);
            }
            if self
                .image_cb
                .as_ref()
                .is_some_and(|record| record.registration.needs_unregister())
            {
                let code = (fns.unregister_image_callback)(context, handle);
                record_cleanup_result(&mut failures, CleanupStep::UnregisterImageCallback, code);
            }
            if self
                .exception_cb
                .as_ref()
                .is_some_and(|record| record.registration.needs_unregister())
            {
                let code = (fns.unregister_exception_callback)(context, handle);
                record_cleanup_result(
                    &mut failures,
                    CleanupStep::UnregisterExceptionCallback,
                    code,
                );
            }
            for record in &self.event_cbs {
                if record.callback.registration.needs_unregister() {
                    let code =
                        (fns.unregister_event_callback)(context, handle, record.name.as_ptr());
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

        if destroyed {
            self.image_cb = None;
            self.exception_cb = None;
            self.event_cbs.clear();
        } else {
            // DestroyHandle failed, so the native side may still retain one
            // or more callback pointers or event-name pointers. Their now
            // empty backing allocations must remain valid indefinitely.
            self.leak_callback_backing();
        }

        // When DestroyHandle fails, the raw native handle is intentionally
        // leaked as well; taking it above guarantees this wrapper never
        // retries teardown.
        let result = if failures.is_empty() {
            Ok(())
        } else {
            Err(CleanupError::new(failures))
        };
        CleanupReport {
            result,
            disposition: Some(if destroyed {
                HandleDisposition::Destroyed
            } else {
                HandleDisposition::Orphaned
            }),
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

    fn drain_callbacks_except_current_exception(&self) {
        if let Some(record) = &self.image_cb
            && let Some(callback) = record.slot.take_callback()
        {
            drop_callback_safely(callback);
        }
        if let Some(record) = &self.exception_cb
            && !record.slot.is_current()
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

    fn abandon_from_restricted_callback_context(&mut self) -> CleanupReport {
        self.stop_accepting_callbacks();
        self.try_drain_callbacks();

        // Native teardown is not documented as safe from image/event callback
        // contexts. Leave the handle and every native-referenced allocation
        // alive instead of releasing borrowed data or the current slot.
        let _ = self.handle.take();
        self.leak_callback_backing();

        CleanupReport {
            result: Err(CleanupError::new(vec![CleanupFailure {
                step: CleanupStep::DrainCallbacks,
                error: MvsError::CallOrder,
            }])),
            disposition: Some(HandleDisposition::Orphaned),
        }
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

    use crate::backend::AcquisitionState;
    use crate::camera::{
        EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
        ImageCallback as ImageCallbackFn,
    };
    use crate::{AccessMode, CleanupStep, Frame, MvsError, sys};

    use super::{
        AcquisitionFns, CallbackFns, CallbackRecord, CallbackSlot, Camera, CleanupFns, EventRecord,
        HandleDisposition, OpenFns, RegistrationState, bool_from_raw, enum_node_from_raw,
        float_node_from_raw, int_node_from_raw, string_from_raw,
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

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum OpenCall {
        CreateHandle,
        OpenDevice,
        DestroyHandle,
    }

    struct FakeOpen {
        calls: Vec<OpenCall>,
        results: VecDeque<c_int>,
        handle: *mut c_void,
    }

    impl FakeOpen {
        fn with_results(results: impl IntoIterator<Item = u32>) -> Self {
            Self {
                calls: Vec::new(),
                results: results.into_iter().map(|result| result as c_int).collect(),
                handle: 1_usize as *mut c_void,
            }
        }

        fn call(&mut self, call: OpenCall) -> c_int {
            self.calls.push(call);
            self.results
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected open call: {call:?}"))
        }
    }

    const FAKE_OPEN_FNS: OpenFns<FakeOpen> = OpenFns {
        create_handle: fake_create_handle,
        open_device: fake_open_device,
        destroy_handle: fake_open_destroy_handle,
    };

    fn fake_create_handle(
        context: &mut FakeOpen,
        handle: *mut *mut c_void,
        device: *const sys::MV_CC_DEVICE_INFO,
    ) -> c_int {
        assert!(!handle.is_null());
        assert!(!device.is_null());
        let code = context.call(OpenCall::CreateHandle);
        if code as u32 == sys::MV_OK {
            // SAFETY: open_with passes writable local handle storage.
            unsafe { *handle = context.handle };
        }
        code
    }

    fn fake_open_device(
        context: &mut FakeOpen,
        handle: *mut c_void,
        access_mode: u32,
        switchover_key: u16,
    ) -> c_int {
        assert_eq!(handle, context.handle);
        assert_eq!(access_mode, AccessMode::Exclusive.raw());
        assert_eq!(switchover_key, 0);
        context.call(OpenCall::OpenDevice)
    }

    fn fake_open_destroy_handle(context: &mut FakeOpen, handle: *mut c_void) -> c_int {
        assert_eq!(handle, context.handle);
        context.call(OpenCall::DestroyHandle)
    }

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

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AcquisitionCall {
        Start,
        Stop,
    }

    #[derive(Default)]
    struct FakeAcquisition {
        calls: Vec<AcquisitionCall>,
        results: VecDeque<c_int>,
    }

    impl FakeAcquisition {
        fn with_results(results: impl IntoIterator<Item = u32>) -> Self {
            Self {
                results: results.into_iter().map(|result| result as c_int).collect(),
                ..Self::default()
            }
        }

        fn call(&mut self, call: AcquisitionCall) -> c_int {
            self.calls.push(call);
            self.results
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected acquisition call: {call:?}"))
        }
    }

    const FAKE_ACQUISITION_FNS: AcquisitionFns<FakeAcquisition> = AcquisitionFns {
        start_grabbing: fake_start_grabbing,
        stop_grabbing: fake_acquisition_stop_grabbing,
    };

    fn fake_start_grabbing(context: &mut FakeAcquisition, _handle: *mut c_void) -> c_int {
        context.call(AcquisitionCall::Start)
    }

    fn fake_acquisition_stop_grabbing(
        context: &mut FakeAcquisition,
        _handle: *mut c_void,
    ) -> c_int {
        context.call(AcquisitionCall::Stop)
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
        assert!(!unsafe { CStr::from_ptr(event_name) }.to_bytes().is_empty());
        context.event_calls.push(EventNativeCall {
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

    fn camera(acquisition: AcquisitionState) -> Camera {
        Camera {
            handle: None,
            acquisition,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
        }
    }

    fn camera_with_handle(acquisition: AcquisitionState) -> Camera {
        let mut camera = camera(acquisition);
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
        record.registration = RegistrationState::Registered;
        record
    }

    fn event_record(name: &str, callback: EventCallbackFn) -> EventRecord {
        EventRecord {
            name: CString::new(name).unwrap(),
            callback: active_record(callback),
        }
    }

    fn camera_with_all_callbacks() -> Camera {
        let mut camera = camera_with_handle(AcquisitionState::Callback);
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

    fn probed_event_callback(drops: Arc<AtomicUsize>) -> EventCallbackFn {
        let probe = DropProbe(drops);
        Box::new(move |_| {
            let _ = &probe;
        })
    }

    fn assert_cleanup_calls(mut camera: Camera, expected: &[CleanupStep]) -> FakeCleanup {
        let mut context = FakeCleanup::with_results(vec![sys::MV_OK; expected.len()]);
        let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut context);
        assert_eq!(report.disposition, Some(HandleDisposition::Destroyed));
        report.result.unwrap();

        assert_eq!(context.calls, expected);
        assert!(context.results.is_empty());
        assert!(camera.handle.is_none());
        context
    }

    #[test]
    fn raw_node_values_convert_without_trusting_native_lengths() {
        let integer = sys::MVCC_INTVALUE_EX {
            nCurValue: 12,
            nMin: 2,
            nMax: 42,
            nInc: 4,
            ..Default::default()
        };
        let integer = int_node_from_raw(&integer);
        assert_eq!(
            (integer.current, integer.min, integer.max, integer.inc),
            (12, 2, 42, 4)
        );

        let float = sys::MVCC_FLOATVALUE {
            fCurValue: 1.5,
            fMin: 0.25,
            fMax: 8.0,
            ..Default::default()
        };
        let float = float_node_from_raw(&float);
        assert_eq!((float.current, float.min, float.max), (1.5, 0.25, 8.0));
        assert!(!bool_from_raw(0));
        assert!(bool_from_raw(-1));

        let mut string = sys::MVCC_STRINGVALUE::default();
        for (target, source) in string.chCurValue.iter_mut().zip(b"camera\0ignored") {
            *target = *source as c_char;
        }
        assert_eq!(string_from_raw(&string), "camera");

        let mut enumeration = sys::MVCC_ENUMVALUE {
            nCurValue: 7,
            nSupportedNum: u32::MAX,
            ..Default::default()
        };
        for (index, value) in enumeration.nSupportValue.iter_mut().enumerate() {
            *value = index as u32;
        }
        let enumeration = enum_node_from_raw(&enumeration);
        assert_eq!(enumeration.current, 7);
        assert_eq!(enumeration.supported.len(), 64);
        assert_eq!(enumeration.supported[63], 63);
    }

    #[test]
    fn open_failures_report_handle_disposition() {
        let device = sys::MV_CC_DEVICE_INFO::default();
        let create = [OpenCall::CreateHandle];
        let open_then_destroy = [
            OpenCall::CreateHandle,
            OpenCall::OpenDevice,
            OpenCall::DestroyHandle,
        ];
        let cases: &[(
            &[u32],
            &[OpenCall],
            u32,
            Option<u32>,
            Option<HandleDisposition>,
        )] = &[
            (
                &[sys::MV_E_RESOURCE],
                &create,
                sys::MV_E_RESOURCE,
                None,
                None,
            ),
            (
                &[sys::MV_OK, sys::MV_E_ACCESS_DENIED, sys::MV_OK],
                &open_then_destroy,
                sys::MV_E_ACCESS_DENIED,
                None,
                Some(HandleDisposition::Destroyed),
            ),
            (
                &[sys::MV_OK, sys::MV_E_ACCESS_DENIED, sys::MV_E_RESOURCE],
                &open_then_destroy,
                sys::MV_E_ACCESS_DENIED,
                Some(sys::MV_E_RESOURCE),
                Some(HandleDisposition::Orphaned),
            ),
        ];

        for &(results, expected_calls, error_code, rollback_code, disposition) in cases {
            let mut context = FakeOpen::with_results(results.iter().copied());
            let failure =
                Camera::open_with(&device, AccessMode::Exclusive, &FAKE_OPEN_FNS, &mut context)
                    .err()
                    .expect("open failure must be returned");

            assert_eq!(context.calls, expected_calls);
            assert_eq!(failure.error.raw_code(), Some(error_code));
            assert_eq!(
                failure.rollback_error.as_ref().and_then(MvsError::raw_code),
                rollback_code
            );
            assert_eq!(failure.disposition, disposition);
        }
    }

    #[test]
    fn image_registration_selects_mode_and_is_immutable_while_grabbing() {
        let mut callbacks = FakeCallbacks::default();
        let old_drops = Arc::new(AtomicUsize::new(0));
        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let handle = camera.handle.unwrap();
        assert_eq!(
            camera.begin_grabbing().unwrap(),
            (handle, AcquisitionState::Polling)
        );

        camera.image_cb = Some(active_record(probed_image_callback(Arc::clone(&old_drops))));
        let old_user = camera.image_cb.as_ref().unwrap().user_data();
        assert_eq!(
            camera.begin_grabbing().unwrap(),
            (handle, AcquisitionState::Callback)
        );
        camera.image_cb.as_mut().unwrap().registration = RegistrationState::Unknown;
        assert!(matches!(camera.begin_grabbing(), Err(MvsError::CallOrder)));
        camera.image_cb.as_mut().unwrap().registration = RegistrationState::Registered;
        camera.acquisition = AcquisitionState::Callback;

        assert!(matches!(
            camera.register_image_callback_with(
                image_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks
            ),
            Err(MvsError::CallOrder)
        ));
        assert!(matches!(
            camera.unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks),
            Err(MvsError::CallOrder)
        ));
        let record = camera.image_cb.as_ref().unwrap();
        assert_eq!(record.user_data(), old_user);
        assert!(record.is_active());
        assert_eq!(old_drops.load(Ordering::SeqCst), 0);
        assert!(callbacks.image_calls.is_empty());

        camera.handle = None;
    }

    #[test]
    fn acquisition_failures_become_unknown_until_stop_recovers() {
        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut acquisition = FakeAcquisition::with_results([
            sys::MV_E_RESOURCE,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_E_RESOURCE,
            sys::MV_OK,
        ]);

        assert!(matches!(
            camera.start_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition),
            Err(MvsError::Resource)
        ));
        assert_eq!(camera.acquisition, AcquisitionState::Unknown);
        assert!(matches!(
            camera.start_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition),
            Err(MvsError::CallOrder)
        ));
        assert_eq!(acquisition.calls, [AcquisitionCall::Start]);

        camera
            .stop_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition)
            .unwrap();
        assert_eq!(camera.acquisition, AcquisitionState::Stopped);

        camera
            .start_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition)
            .unwrap();
        assert_eq!(camera.acquisition, AcquisitionState::Polling);
        assert!(matches!(
            camera.stop_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition),
            Err(MvsError::Resource)
        ));
        assert_eq!(camera.acquisition, AcquisitionState::Unknown);
        camera
            .stop_grabbing_with(&FAKE_ACQUISITION_FNS, &mut acquisition)
            .unwrap();
        assert_eq!(camera.acquisition, AcquisitionState::Stopped);

        camera.handle = None;
    }

    #[test]
    fn image_callback_uses_the_current_closure_across_its_lifecycle() {
        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut callbacks = FakeCallbacks::default();

        let old_calls = Arc::new(AtomicUsize::new(0));
        let current_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&old_calls);
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let counter = Arc::clone(&current_calls);
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        assert_eq!(Arc::strong_count(&old_calls), 1);
        assert_eq!(callbacks.image_calls.len(), 1);
        callbacks.invoke_image(0);
        assert_eq!(old_calls.load(Ordering::SeqCst), 0);
        assert_eq!(current_calls.load(Ordering::SeqCst), 1);

        camera
            .unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        callbacks.invoke_image(0);
        assert_eq!(current_calls.load(Ordering::SeqCst), 1);

        let reregistered_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&reregistered_calls);
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        callbacks.invoke_image(0);
        assert_eq!(reregistered_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            callbacks
                .image_calls
                .iter()
                .filter(|call| call.callback.is_some())
                .count(),
            2
        );

        camera.handle = None;
    }

    #[test]
    fn failed_registration_allows_sync_delivery_then_silences_late_callbacks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&drops));
        let counter = Arc::clone(&calls);
        let mut camera = camera_with_handle(AcquisitionState::Stopped);
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
        assert_eq!(camera.acquisition, AcquisitionState::Stopped);
        assert_eq!(
            camera.image_cb.as_ref().unwrap().registration,
            RegistrationState::Unknown
        );
        assert!(camera.normal_handle().is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!camera.image_cb.as_ref().unwrap().slot.is_active());

        callbacks.invoke_image(0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            camera.register_image_callback_with(
                image_callback(),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            ),
            Err(MvsError::CallOrder)
        ));
        assert_eq!(callbacks.image_calls.len(), 1);

        assert_cleanup_calls(
            camera,
            &[
                CleanupStep::UnregisterImageCallback,
                CleanupStep::CloseDevice,
                CleanupStep::DestroyHandle,
            ],
        );
    }

    #[test]
    fn failed_unregister_marks_only_callback_unknown_and_can_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&drops));
        let counter = Arc::clone(&calls);
        let mut camera = camera_with_handle(AcquisitionState::Stopped);
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
        callbacks
            .results
            .extend([sys::MV_E_RESOURCE as c_int, sys::MV_OK as c_int]);

        assert!(matches!(
            camera.unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks),
            Err(MvsError::Resource)
        ));
        let record = camera.image_cb.as_ref().unwrap();
        assert_eq!(record.user_data() as usize, user);
        assert_eq!(record.registration, RegistrationState::Unknown);
        assert!(!record.slot.is_active());
        assert_eq!(camera.acquisition, AcquisitionState::Stopped);
        assert!(camera.normal_handle().is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        callbacks.invoke_image(0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        camera
            .unregister_image_callback_with(&FAKE_CALLBACK_FNS, &mut callbacks)
            .unwrap();
        assert_eq!(
            camera.image_cb.as_ref().unwrap().registration,
            RegistrationState::Unregistered
        );
        assert_eq!(callbacks.image_calls.len(), 3);
        assert!(
            callbacks.image_calls[1..]
                .iter()
                .all(|call| call.callback.is_none() && call.user == 0)
        );
        camera.handle = None;
    }

    #[test]
    fn cleanup_drains_callbacks_runs_every_step_and_then_becomes_a_noop() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut camera = camera_with_handle(AcquisitionState::Unknown);
        camera.image_cb = Some(active_record(probed_image_callback(Arc::clone(&drops))));
        camera.exception_cb = Some(active_record(Box::new({
            let probe = DropProbe(Arc::clone(&drops));
            move |_| {
                let _ = &probe;
            }
        })));
        for name in ["ExposureEnd", "LineStart"] {
            camera.event_cbs.push(event_record(
                name,
                probed_event_callback(Arc::clone(&drops)),
            ));
        }
        let mut context = FakeCleanup::with_results_and_probe(
            vec![sys::MV_OK; FULL_CLEANUP_STEPS.len()],
            Arc::clone(&drops),
        );

        let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut context);
        assert_eq!(report.disposition, Some(HandleDisposition::Destroyed));
        report.result.unwrap();

        assert_eq!(context.calls, FULL_CLEANUP_STEPS);
        assert_eq!(context.event_names, ["ExposureEnd", "LineStart"]);
        assert_eq!(drops.load(Ordering::SeqCst), 4);
        assert!(context.drops_seen.iter().all(|drops| *drops == 4));

        let mut unexpected_call = FakeCleanup::default();
        let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut unexpected_call);
        assert_eq!(report.disposition, None);
        report.result.unwrap();
        assert!(unexpected_call.calls.is_empty());
    }

    #[test]
    fn cleanup_aggregates_all_failures_in_call_order_and_reaches_destroy() {
        let mut camera = camera_with_all_callbacks();
        let mut context = FakeCleanup::with_results([
            sys::MV_E_RESOURCE,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_OK,
            sys::MV_E_HANDLE,
        ]);

        let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut context);
        assert_eq!(report.disposition, Some(HandleDisposition::Orphaned));
        let error = report.result.unwrap_err();

        assert_eq!(context.calls, FULL_CLEANUP_STEPS);
        assert_eq!(
            error
                .failures()
                .iter()
                .map(|failure| failure.step)
                .collect::<Vec<_>>(),
            [CleanupStep::StopGrabbing, CleanupStep::DestroyHandle]
        );
    }

    struct SendCamera(Camera);

    // SAFETY: this mirrors the public Camera Send contract solely for moving a
    // synthetic backend camera into the callback-owned test holder.
    unsafe impl Send for SendCamera {}

    type Shared<T> = Arc<Mutex<Option<T>>>;
    type CleanupTrace = (Vec<CleanupStep>, Vec<CleanupStep>);

    fn shared<T>() -> Shared<T> {
        Arc::new(Mutex::new(None))
    }

    fn store_shared<T>(slot: &Shared<T>, value: T) {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
    }

    fn take_shared<T>(slot: &Shared<T>, message: &str) -> T {
        slot.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect(message)
    }

    fn take_camera(holder: &Shared<SendCamera>, message: &str) -> Camera {
        take_shared(holder, message).0
    }

    fn cleanup_trace(
        camera: &mut Camera,
        results: impl IntoIterator<Item = u32>,
    ) -> (HandleDisposition, CleanupTrace) {
        let mut native = FakeCleanup::with_results(results);
        let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut native);
        let disposition = report.disposition.expect("camera owned a handle");
        let failures = match report.result {
            Ok(()) => Vec::new(),
            Err(error) => error
                .failures()
                .iter()
                .map(|failure| failure.step)
                .collect(),
        };
        (disposition, (native.calls, failures))
    }

    #[test]
    fn exception_callback_cleanup_closes_and_destroys_then_defers_current_slot() {
        type Outcome = (Vec<CleanupStep>, Vec<usize>, bool, usize);

        let camera_holder = shared();
        let outcome: Shared<Outcome> = shared();
        let slot_weak_holder: Shared<std::sync::Weak<CallbackSlot<ExceptionCallbackFn>>> = shared();
        let drops = Arc::new(AtomicUsize::new(0));
        let current_probe = DropProbe(Arc::clone(&drops));
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);
        let weak_for_callback = Arc::clone(&slot_weak_holder);
        let drops_for_callback = Arc::clone(&drops);

        let mut camera = camera_with_handle(AcquisitionState::Callback);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_exception_callback_with(
                Box::new(move |_| {
                    let _ = &current_probe;
                    let mut camera =
                        take_camera(&holder_for_callback, "camera installed before callback");

                    let mut native = FakeCleanup::with_results_and_probe(
                        [sys::MV_OK, sys::MV_OK],
                        Arc::clone(&drops_for_callback),
                    );
                    let report = camera.cleanup_with(&FAKE_CLEANUP_FNS, &mut native);
                    assert_eq!(report.disposition, Some(HandleDisposition::Destroyed));
                    report.result.unwrap();
                    let slot_is_pinned = take_shared(
                        &weak_for_callback,
                        "slot weak reference installed before callback",
                    )
                    .upgrade()
                    .is_some();
                    store_shared(
                        &outcome_for_callback,
                        (
                            native.calls,
                            native.drops_seen,
                            slot_is_pinned,
                            drops_for_callback.load(Ordering::SeqCst),
                        ),
                    );
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        camera.image_cb = Some(active_record(probed_image_callback(Arc::clone(&drops))));
        camera.event_cbs.push(event_record(
            "ExposureEnd",
            probed_event_callback(Arc::clone(&drops)),
        ));

        let slot_weak = Arc::downgrade(&camera.exception_cb.as_ref().unwrap().slot);
        store_shared(&slot_weak_holder, slot_weak.clone());
        store_shared(&camera_holder, SendCamera(camera));

        callbacks.invoke_exception(0);

        let (native_calls, drops_seen, slot_was_pinned, drops_in_callback) =
            take_shared(&outcome, "callback recorded cleanup outcome");
        assert_eq!(
            native_calls,
            [CleanupStep::CloseDevice, CleanupStep::DestroyHandle]
        );
        assert_eq!(drops_seen, [2, 2]);
        assert!(slot_was_pinned);
        assert_eq!(drops_in_callback, 2);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
        assert!(slot_weak.upgrade().is_none());
    }

    #[test]
    fn exception_callback_destroy_failure_keeps_backing_and_silences_late_calls() {
        let camera_holder = shared();
        let outcome: Shared<CleanupTrace> = shared();
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let callback_drops = Arc::new(AtomicUsize::new(0));
        let probe = DropProbe(Arc::clone(&callback_drops));
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);
        let calls_for_callback = Arc::clone(&callback_calls);

        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_exception_callback_with(
                Box::new(move |_| {
                    let _ = &probe;
                    calls_for_callback.fetch_add(1, Ordering::SeqCst);
                    let mut camera =
                        take_camera(&holder_for_callback, "camera installed before callback");
                    let (disposition, trace) =
                        cleanup_trace(&mut camera, [sys::MV_OK, sys::MV_E_RESOURCE]);
                    assert_eq!(disposition, HandleDisposition::Orphaned);
                    store_shared(&outcome_for_callback, trace);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        let slot_weak = Arc::downgrade(&camera.exception_cb.as_ref().unwrap().slot);
        store_shared(&camera_holder, SendCamera(camera));
        callbacks.invoke_exception(0);

        let (native_calls, failure_steps) =
            take_shared(&outcome, "callback recorded cleanup outcome");
        assert_eq!(
            native_calls,
            [CleanupStep::CloseDevice, CleanupStep::DestroyHandle]
        );
        assert_eq!(failure_steps, [CleanupStep::DestroyHandle]);
        assert_eq!(callback_calls.load(Ordering::SeqCst), 1);
        assert_eq!(callback_drops.load(Ordering::SeqCst), 1);
        assert!(slot_weak.upgrade().is_some());

        // The failed destroy may leave pUser registered. Its backing is
        // intentionally leaked, but the closure has been removed.
        callbacks.invoke_exception(0);
        assert_eq!(callback_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn image_callback_cleanup_abandons_without_native_teardown() {
        let camera_holder = shared();
        let outcome: Shared<CleanupTrace> = shared();
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);
        let calls_for_callback = Arc::clone(&callback_calls);

        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    calls_for_callback.fetch_add(1, Ordering::SeqCst);
                    let mut camera =
                        take_camera(&holder_for_callback, "camera installed before callback");
                    let mut rejected_native = FakeCallbacks::default();
                    assert!(matches!(
                        camera.unregister_image_callback_with(
                            &FAKE_CALLBACK_FNS,
                            &mut rejected_native,
                        ),
                        Err(MvsError::CallOrder)
                    ));
                    assert!(rejected_native.image_calls.is_empty());
                    let (disposition, trace) =
                        cleanup_trace(&mut camera, std::iter::repeat_n(sys::MV_OK, 8));
                    assert_eq!(disposition, HandleDisposition::Orphaned);
                    store_shared(&outcome_for_callback, trace);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        store_shared(&camera_holder, SendCamera(camera));
        callbacks.invoke_image(0);

        let (native_calls, failure_steps) =
            take_shared(&outcome, "callback recorded cleanup outcome");
        assert!(native_calls.is_empty());
        assert_eq!(failure_steps, [CleanupStep::DrainCallbacks]);

        // DestroyHandle was deliberately skipped, so a late native call may
        // still reach the leaked backing but must observe the disabled slot.
        callbacks.invoke_image(0);
        assert_eq!(callback_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn nested_exception_inside_image_callback_remains_conservative() {
        let camera_holder = shared();
        let outcome: Shared<CleanupTrace> = shared();
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);

        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_exception_callback_with(
                Box::new(move |_| {
                    let mut camera = take_camera(
                        &holder_for_callback,
                        "camera installed before nested callback",
                    );
                    let (disposition, trace) =
                        cleanup_trace(&mut camera, std::iter::repeat_n(sys::MV_OK, 8));
                    assert_eq!(disposition, HandleDisposition::Orphaned);
                    store_shared(&outcome_for_callback, trace);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();
        let exception_call = callbacks.exception_calls[0];
        camera
            .register_image_callback_with(
                Box::new(move |_| {
                    let callback = exception_call
                        .callback
                        .expect("expected an exception registration");
                    // SAFETY: the Camera containing this live exception slot
                    // remains in the holder until the nested callback takes it.
                    unsafe { callback(7, exception_call.user as *mut c_void) };
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        store_shared(&camera_holder, SendCamera(camera));
        callbacks.invoke_image(0);

        let (native_calls, failure_steps) =
            take_shared(&outcome, "nested callback recorded cleanup outcome");
        assert!(native_calls.is_empty());
        assert_eq!(failure_steps, [CleanupStep::DrainCallbacks]);
    }

    #[test]
    fn event_callback_cleanup_abandons_without_native_teardown() {
        let camera_holder = shared();
        let outcome: Shared<CleanupTrace> = shared();
        let holder_for_callback = Arc::clone(&camera_holder);
        let outcome_for_callback = Arc::clone(&outcome);

        let mut camera = camera_with_handle(AcquisitionState::Stopped);
        let mut callbacks = FakeCallbacks::default();
        camera
            .register_event_callback_with(
                "ExposureEnd",
                Box::new(move |_| {
                    let mut camera =
                        take_camera(&holder_for_callback, "camera installed before callback");
                    let (disposition, trace) =
                        cleanup_trace(&mut camera, std::iter::repeat_n(sys::MV_OK, 8));
                    assert_eq!(disposition, HandleDisposition::Orphaned);
                    store_shared(&outcome_for_callback, trace);
                }),
                &FAKE_CALLBACK_FNS,
                &mut callbacks,
            )
            .unwrap();

        store_shared(&camera_holder, SendCamera(camera));
        callbacks.invoke_event(0);

        let (native_calls, failure_steps) =
            take_shared(&outcome, "callback recorded cleanup outcome");
        assert!(native_calls.is_empty());
        assert_eq!(failure_steps, [CleanupStep::DrainCallbacks]);
    }
}
