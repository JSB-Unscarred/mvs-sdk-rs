//! Windows 相机 handle、采集状态与节点访问。

use std::ffi::CString;
use std::fmt;
use std::os::raw::{c_float, c_int, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::error::check;
use crate::library::RuntimeCore;
use crate::sys;
use crate::text::{SdkText, sdk_bytes_from_chars};
use crate::{AccessMode, CleanupError, EnumValue, FloatValue, IntValue, MvsError, MvsResult};

use super::callback::{
    CallbackSlot, event_trampoline, exception_trampoline, image_trampoline, in_callback,
};
use super::{DeviceInfo, FrameGuard};

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

/// Camera 单独持有 callback slot；Box 地址稳定到 DestroyHandle。
///
/// slot 在 `Camera::open` 时就位并复用，注册状态因此只由 `registered` 一个字段表达。
struct CallbackRecord<C> {
    slot: Box<CallbackSlot<C>>,
    registered: bool,
}

impl<C: Clone> CallbackRecord<C> {
    fn new() -> Self {
        Self {
            slot: Box::new(CallbackSlot::new()),
            registered: false,
        }
    }

    fn user_data(&self) -> *mut c_void {
        std::ptr::from_ref(self.slot.as_ref()).cast_mut().cast()
    }

    fn register(
        &mut self,
        callback: C,
        native_register: impl FnOnce(*mut c_void) -> c_int,
    ) -> MvsResult<()> {
        if self.registered {
            return Err(MvsError::InvalidState("callback is already registered"));
        }
        self.slot.set(callback);

        if let Err(error) = check(native_register(self.user_data())) {
            // 仅 native 成功才提交 registered；失败时清空 closure 并保留未注册 slot。
            self.slot.clear();
            return Err(error);
        }

        self.registered = true;
        Ok(())
    }

    /// 未注册时直接成功，teardown 路径可无条件调用。
    fn unregister(&mut self, native_unregister: impl FnOnce() -> c_int) -> MvsResult<()> {
        if !self.registered {
            return Ok(());
        }

        check(native_unregister())?;
        self.registered = false;
        self.slot.clear();
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.registered
    }

    fn silence(&self) {
        self.slot.clear();
    }

    /// DestroyHandle 失败后遗留空 slot，防止 native `pUser` 悬垂。
    fn leak_slot(self) {
        let _ = Box::leak(self.slot);
    }
}

struct EventRecord {
    name: CString,
    callback: CallbackRecord<EventCallbackFn>,
}

/// native handle 只有 backend Camera 一个 owner；公开 Camera 通过 `!Sync` 限制并发调用。
struct NativeHandle(NonNull<c_void>);

// SAFETY: 厂商示例会把 handle 交给工作线程；该包装只移动唯一 owner。
unsafe impl Send for NativeHandle {}

impl NativeHandle {
    fn new(raw: *mut c_void, runtime: &RuntimeCore) -> Option<Self> {
        NonNull::new(raw).map(|raw| {
            runtime.native_handle_created();
            Self(raw)
        })
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// 计数只在 SDK 确认 DestroyHandle 成功后解除。
    fn destroy(self, runtime: &RuntimeCore) -> MvsResult<()> {
        // SAFETY: 本值是该 native handle 的唯一 Rust owner。
        check(unsafe { sys::MV_CC_DestroyHandle(self.as_ptr()) })?;
        runtime.native_handle_destroyed();
        Ok(())
    }
}

/// 已打开的 MVS 相机及其局部资源。
pub(crate) struct Camera {
    runtime: Arc<RuntimeCore>,
    handle: Option<NativeHandle>,
    grabbing: bool,
    image_cb: CallbackRecord<ImageCallbackFn>,
    exception_cb: CallbackRecord<ExceptionCallbackFn>,
    event_cbs: Vec<EventRecord>,
}

impl Camera {
    /// 创建并打开 handle；OpenDevice 失败时回滚销毁，销毁也失败时保留两项错误。
    pub(crate) fn open(
        runtime: Arc<RuntimeCore>,
        device: DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> MvsResult<Self> {
        reject_callback_context()?;
        let mut raw_handle = std::ptr::null_mut();
        // SAFETY: 输出地址可写，device 是 Rust-owned SDK 结构体快照。
        // CreateHandle 失败时不写出 handle，无需回滚。
        check(unsafe { sys::MV_CC_CreateHandle(&mut raw_handle, device.raw()) })?;
        let handle =
            NativeHandle::new(raw_handle, &runtime).ok_or(MvsError::NullHandleAfterCreate)?;

        // SAFETY: handle 来自 CreateHandle，访问模式与切换 key 直接转发。
        let open =
            check(unsafe { sys::MV_CC_OpenDevice(handle.as_ptr(), mode.raw(), switchover_key) });
        if let Err(error) = open {
            // SAFETY: OpenDevice 失败后 handle 仍由本函数唯一持有。
            return Err(match handle.destroy(&runtime) {
                Ok(()) => error,
                Err(destroy) => MvsError::OpenRollback {
                    open: Box::new(error),
                    destroy: Box::new(destroy),
                },
            });
        }

        Ok(Self {
            runtime,
            handle: Some(handle),
            grabbing: false,
            image_cb: CallbackRecord::new(),
            exception_cb: CallbackRecord::new(),
            event_cbs: Vec::new(),
        })
    }

    fn handle(&self) -> MvsResult<*mut c_void> {
        self.handle
            .as_ref()
            .map(NativeHandle::as_ptr)
            .ok_or(MvsError::InvalidState("camera handle is unavailable"))
    }

    fn stopped_handle(&self) -> MvsResult<*mut c_void> {
        if self.grabbing {
            return Err(MvsError::InvalidState("camera must be stopped"));
        }
        self.handle()
    }

    fn handle_and_key(&self, key: &str) -> MvsResult<(*mut c_void, CString)> {
        Ok((self.handle()?, CString::new(key)?))
    }

    /// 借出 handle 只用于尚未覆盖的厂商接口；调用方不得改变本包装维护的状态。
    pub(crate) fn as_raw_handle(&self) -> *mut c_void {
        self.handle
            .as_ref()
            .map_or(std::ptr::null_mut(), NativeHandle::as_ptr)
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.handle.as_ref().is_some_and(|handle| {
            // SAFETY: handle 在本 Camera 生命周期内有效。
            unsafe { sys::MV_CC_IsDeviceConnected(handle.as_ptr()) != 0 }
        })
    }

    /// 启动取流；image callback 是否注册决定 callback 或 polling 模式。
    pub(crate) fn start_grabbing(&mut self) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.stopped_handle()?;

        // SAFETY: handle 已打开，且本地状态为 stopped。
        check(unsafe { sys::MV_CC_StartGrabbing(handle) })?;
        self.grabbing = true;
        Ok(())
    }

    /// 未取流时直接成功；仅在 native 调用成功后更新本地状态，失败时允许原操作重试。
    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        reject_callback_context()?;
        if !self.grabbing {
            return Ok(());
        }
        let handle = self.handle()?;
        // SAFETY: 本地状态记录当前正在取流。
        check(unsafe { sys::MV_CC_StopGrabbing(handle) })?;
        self.grabbing = false;
        Ok(())
    }

    /// polling 模式获取的 buffer 由 FrameGuard 唯一负责归还。
    pub(crate) fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        if !self.grabbing || self.image_cb.is_active() {
            return Err(MvsError::InvalidState(
                "polling requires active grabbing without an image callback",
            ));
        }
        let handle = self.handle()?;
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw 是可写输出结构体，handle 正在 polling 取流。
        check(unsafe { sys::MV_CC_GetImageBuffer(handle, &mut raw, timeout_ms) })?;
        Ok(FrameGuard::new(handle, raw))
    }

    /// image callback 的注册与注销都要求停止取流，匹配厂商示例顺序。
    pub(crate) fn register_image_callback(&mut self, callback: ImageCallbackFn) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.stopped_handle()?;
        self.image_cb.register(callback, |user| {
            // SAFETY: Box slot 地址稳定；autoFree=true 限定 Frame 借用期。
            unsafe { sys::MV_CC_RegisterImageCallBackEx2(handle, Some(image_trampoline), user, 1) }
        })
    }

    /// 未注册时直接成功，因此幂等判断先于"要求停止取流"的前置条件。
    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        reject_callback_context()?;
        if !self.image_cb.is_active() {
            return Ok(());
        }
        let handle = self.stopped_handle()?;
        self.image_cb.unregister(|| {
            // SAFETY: 官方 Ex2 示例以 null callback/user 注销，autoFree 仍传 true。
            unsafe { sys::MV_CC_RegisterImageCallBackEx2(handle, None, std::ptr::null_mut(), 1) }
        })
    }

    pub(crate) fn get_int(&self, key: &str) -> MvsResult<IntValue> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_INTVALUE_EX::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetIntValueEx(handle, key.as_ptr(), &mut value) })?;
        Ok(IntValue {
            current: value.nCurValue,
            min: value.nMin,
            max: value.nMax,
            inc: value.nInc,
        })
    }

    pub(crate) fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetIntValueEx(handle, key.as_ptr(), value) })
    }

    pub(crate) fn get_enum(&self, key: &str) -> MvsResult<EnumValue> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_ENUMVALUE_EX::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetEnumValueEx(handle, key.as_ptr(), &mut value) })?;
        let len = (value.nSupportedNum as usize).min(value.nSupportValue.len());
        Ok(EnumValue {
            current: value.nCurValue,
            supported: value.nSupportValue[..len].to_vec(),
        })
    }

    pub(crate) fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetEnumValue(handle, key.as_ptr(), value) })
    }

    pub(crate) fn set_enum_symbolic(&self, key: &str, value: &str) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        let value = CString::new(value)?;
        // SAFETY: 两个字符串在调用期间有效。
        check(unsafe { sys::MV_CC_SetEnumValueByString(handle, key.as_ptr(), value.as_ptr()) })
    }

    pub(crate) fn get_float(&self, key: &str) -> MvsResult<FloatValue> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_FLOATVALUE::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetFloatValue(handle, key.as_ptr(), &mut value) })?;
        Ok(FloatValue {
            current: value.fCurValue,
            min: value.fMin,
            max: value.fMax,
        })
    }

    pub(crate) fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetFloatValue(handle, key.as_ptr(), value as c_float) })
    }

    pub(crate) fn get_bool(&self, key: &str) -> MvsResult<bool> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value: sys::bool_ = 0;
        // SAFETY: value 是可写输出参数。
        check(unsafe { sys::MV_CC_GetBoolValue(handle, key.as_ptr(), &mut value) })?;
        Ok(value != 0)
    }

    pub(crate) fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        let value: sys::bool_ = if value { 1 } else { 0 };
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetBoolValue(handle, key.as_ptr(), value) })
    }

    pub(crate) fn get_string(&self, key: &str) -> MvsResult<SdkText> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_STRINGVALUE::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetStringValue(handle, key.as_ptr(), &mut value) })?;
        Ok(SdkText::from_sdk_bytes(
            sdk_bytes_from_chars(&value.chCurValue).to_vec(),
        ))
    }

    pub(crate) fn set_string(&self, key: &str, value: &[u8]) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        let value = CString::new(value)?;
        // SAFETY: 两个字符串在调用期间有效。
        check(unsafe { sys::MV_CC_SetStringValue(handle, key.as_ptr(), value.as_ptr()) })
    }

    pub(crate) fn exec_command(&self, key: &str) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetCommandValue(handle, key.as_ptr()) })
    }

    pub(crate) fn register_exception_callback(
        &mut self,
        callback: ExceptionCallbackFn,
    ) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.handle()?;
        self.exception_cb.register(callback, |user| {
            // SAFETY: Box slot 地址稳定并匹配 exception trampoline 类型。
            unsafe {
                sys::MV_CC_RegisterExceptionCallBack(handle, Some(exception_trampoline), user)
            }
        })
    }

    pub(crate) fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.handle()?;
        self.exception_cb.unregister(|| {
            // SAFETY: CHM 说明 null callback/user 注销 exception callback。
            unsafe { sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut()) }
        })
    }

    /// event name 动态，因此按名字查找或新建 record；注册失败保留未注册 record 供重试。
    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        callback: EventCallbackFn,
    ) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.handle()?;
        let name = CString::new(event_name)?;
        // 先结束对 event_cbs 的不可变借用，再决定复用还是新建 record。
        let existing = self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str());
        let index = match existing {
            Some(index) => index,
            None => {
                self.event_cbs.push(EventRecord {
                    name,
                    callback: CallbackRecord::new(),
                });
                self.event_cbs.len() - 1
            }
        };

        let EventRecord {
            name,
            callback: record,
        } = &mut self.event_cbs[index];
        let name_ptr = name.as_ptr();
        record.register(callback, |user| {
            // SAFETY: event name 和 Box slot 均由 EventRecord 稳定持有。
            unsafe {
                sys::MV_CC_RegisterEventCallBackEx(handle, name_ptr, Some(event_trampoline), user)
            }
        })
    }

    pub(crate) fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        reject_callback_context()?;
        let handle = self.handle()?;
        let name = CString::new(event_name)?;
        let Some(index) = self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str())
        else {
            return Ok(());
        };

        let EventRecord {
            name,
            callback: record,
        } = &mut self.event_cbs[index];
        let name_ptr = name.as_ptr();
        record.unregister(|| {
            // SAFETY: CHM 说明 null callback/user 注销 named event callback。
            unsafe {
                sys::MV_CC_RegisterEventCallBackEx(handle, name_ptr, None, std::ptr::null_mut())
            }
        })
    }

    pub(crate) fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        let (handle, name) = self.handle_and_key(event_name)?;
        // SAFETY: name 在调用期间有效。
        check(unsafe { sys::MV_CC_EventNotificationOn(handle, name.as_ptr()) })
    }

    pub(crate) fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        let (handle, name) = self.handle_and_key(event_name)?;
        // SAFETY: name 在调用期间有效。
        check(unsafe { sys::MV_CC_EventNotificationOff(handle, name.as_ptr()) })
    }

    /// 尝试完整 teardown，分别保留前序首错与 DestroyHandle 错误。
    /// callback 线程上的 close/Drop 会终止进程：厂商禁止从 callback 重入生命周期接口。
    pub(crate) fn cleanup(&mut self) -> Result<(), CleanupError> {
        if in_callback() {
            std::process::abort();
        }

        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        self.silence_callbacks();
        let raw_handle = handle.as_ptr();
        let mut prior_error = None;

        if self.grabbing {
            // SAFETY: handle 尚由本 Camera 持有；失败不阻止后续 teardown。
            record_first_error(
                &mut prior_error,
                "MV_CC_StopGrabbing",
                check(unsafe { sys::MV_CC_StopGrabbing(raw_handle) }),
            );
        }
        if self.image_cb.is_active() {
            // SAFETY: 官方 Ex2 示例以 null callback/user 注销。
            record_first_error(
                &mut prior_error,
                "MV_CC_RegisterImageCallBackEx2(NULL)",
                check(unsafe {
                    sys::MV_CC_RegisterImageCallBackEx2(raw_handle, None, std::ptr::null_mut(), 1)
                }),
            );
        }
        if self.exception_cb.is_active() {
            // SAFETY: null callback/user 注销 exception callback。
            record_first_error(
                &mut prior_error,
                "MV_CC_RegisterExceptionCallBack(NULL)",
                check(unsafe {
                    sys::MV_CC_RegisterExceptionCallBack(raw_handle, None, std::ptr::null_mut())
                }),
            );
        }
        for record in &self.event_cbs {
            if record.callback.is_active() {
                // SAFETY: event name 仍由 record 持有。
                record_first_error(
                    &mut prior_error,
                    "MV_CC_RegisterEventCallBackEx(NULL)",
                    check(unsafe {
                        sys::MV_CC_RegisterEventCallBackEx(
                            raw_handle,
                            record.name.as_ptr(),
                            None,
                            std::ptr::null_mut(),
                        )
                    }),
                );
            }
        }

        // SAFETY: teardown 继续遵循 CloseDevice → DestroyHandle。
        record_first_error(
            &mut prior_error,
            "MV_CC_CloseDevice",
            check(unsafe { sys::MV_CC_CloseDevice(raw_handle) }),
        );
        let destroy_error = handle.destroy(&self.runtime).err();
        let destroyed = destroy_error.is_none();

        self.grabbing = false;
        if destroyed {
            self.event_cbs.clear();
        } else {
            // Destroy 失败时 native 仍可能保存 pUser，只遗留对应的空 Box slot。
            self.leak_callback_slots();
        }

        if prior_error.is_none() && destroy_error.is_none() {
            Ok(())
        } else {
            Err(CleanupError::new(prior_error, destroy_error, destroyed))
        }
    }

    fn silence_callbacks(&self) {
        self.image_cb.silence();
        self.exception_cb.silence();
        for record in &self.event_cbs {
            record.callback.silence();
        }
    }

    /// DestroyHandle 失败时遗留 native 曾持有指针的空 slot；
    /// 换入的空 record 不再注册，随 Camera 一起释放。
    fn leak_callback_slots(&mut self) {
        std::mem::replace(&mut self.image_cb, CallbackRecord::new()).leak_slot();
        std::mem::replace(&mut self.exception_cb, CallbackRecord::new()).leak_slot();
        for record in self.event_cbs.drain(..) {
            record.callback.leak_slot();
        }
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = if self.handle.is_some() {
            if self.grabbing { "Grabbing" } else { "Open" }
        } else {
            "Closed"
        };
        let mut debug = f.debug_struct("Camera");
        debug
            .field("handle", &self.as_raw_handle())
            .field("state", &state);
        if self.grabbing {
            let mode = if self.image_cb.is_active() {
                "Callback"
            } else {
                "Polling"
            };
            debug.field("acquisition_mode", &mode);
        }
        debug
            .field("image_cb", &self.image_cb.is_active())
            .field("exception_cb", &self.exception_cb.is_active())
            .field(
                "event_cbs",
                &self
                    .event_cbs
                    .iter()
                    .filter(|record| record.callback.is_active())
                    .count(),
            )
            .finish()
    }
}

impl Drop for Camera {
    /// native handle 与 callback slot 属于 backend Camera，Drop 负责局部兜底清理。
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// 拒绝从 SDK callback 线程重入。
///
/// 判据：只有改变 handle 生命周期或取流/callback 注册状态的接口需要拒绝
/// （Create/Open/Start/Stop/Register\*/Close/Destroy），厂商禁止在 callback 中重入它们。
/// 纯 GenICam 节点读写不受此限，`MV_CC_EventNotificationOn/Off` 只写设备寄存器，
/// 与 `set_enum` 同类，因此不调用本函数。
fn reject_callback_context() -> MvsResult<()> {
    if in_callback() {
        Err(MvsError::InvalidState(
            "camera lifecycle operations cannot run from an MVS callback",
        ))
    } else {
        Ok(())
    }
}

/// 记录 DestroyHandle 前首个失败的清理操作，后续清理仍继续。
fn record_first_error(
    first_error: &mut Option<(&'static str, MvsError)>,
    operation: &'static str,
    result: MvsResult<()>,
) {
    if let Err(error) = result
        && first_error.is_none()
    {
        *first_error = Some((operation, error));
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_int;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::camera::ExceptionCallback;
    use crate::{MvsError, sys};

    use super::{CallbackRecord, exception_trampoline};

    // 一个状态测试覆盖同步派发、失败回滚、重复注册、注销幂等和注销后重注册。
    #[test]
    fn callback_record_registration_contract() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut record = CallbackRecord::<ExceptionCallback>::new();
        let user = record.user_data();

        let failed_calls = Arc::clone(&calls);
        let failed_callback: ExceptionCallback = Arc::new(move |_| {
            failed_calls.fetch_add(1, Ordering::SeqCst);
        });
        let error = record
            .register(failed_callback, |registered_user| {
                assert_eq!(registered_user, user);
                // SAFETY: slot 由 record 持有，注册期间地址稳定。
                unsafe { exception_trampoline(1, registered_user) };
                sys::MV_E_PARAMETER as c_int
            })
            .expect_err("native 注册失败应返回原错误");

        assert_eq!(error.raw_code(), Some(sys::MV_E_PARAMETER));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!record.is_active());
        // SAFETY: record 仍持有已清空的 slot。
        unsafe { exception_trampoline(2, user) };
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let callback_calls = Arc::clone(&calls);
        let callback: ExceptionCallback = Arc::new(move |_| {
            callback_calls.fetch_add(1, Ordering::SeqCst);
        });
        record
            .register(Arc::clone(&callback), |registered_user| {
                // SAFETY: slot 在同步派发期间地址稳定。
                unsafe { exception_trampoline(3, registered_user) };
                sys::MV_OK as c_int
            })
            .expect("注册应成功");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(record.is_active());
        assert!(matches!(
            record.register(Arc::clone(&callback), |_| sys::MV_OK as c_int),
            Err(MvsError::InvalidState(_))
        ));

        record
            .unregister(|| sys::MV_OK as c_int)
            .expect("注销应成功");
        // SAFETY: record 仍持有已清空的 slot。
        unsafe { exception_trampoline(4, user) };
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        record
            .unregister(|| panic!("未注册时不应调用 native"))
            .expect("注销应幂等");

        record
            .register(callback, |registered_user| {
                assert_eq!(registered_user, user);
                sys::MV_OK as c_int
            })
            .expect("注销后应可复用同一 slot 重注册");
        // SAFETY: record 持有重新安装 closure 的 slot。
        unsafe { exception_trampoline(5, user) };
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        record
            .unregister(|| sys::MV_OK as c_int)
            .expect("测试清理应成功");
    }
}
