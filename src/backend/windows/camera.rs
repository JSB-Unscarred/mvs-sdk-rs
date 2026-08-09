//! Windows 相机 handle、采集状态与节点访问。

use std::ffi::CString;
use std::os::raw::{c_float, c_int, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::error::check;
use crate::sys;
use crate::{AccessMode, EnumValue, FloatValue, IntValue, MvsError, MvsResult};

use super::callback::{CallbackSlot, event_trampoline, exception_trampoline, image_trampoline};
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

/// callback slot 由 Camera 与 SDK raw token 共同持有，地址稳定到 DestroyHandle。
struct CallbackRecord<C> {
    slot: Arc<CallbackSlot<C>>,
    registered: bool,
    native_token: bool,
}

impl<C> CallbackRecord<C> {
    fn new() -> Self {
        Self {
            slot: Arc::new(CallbackSlot::new()),
            registered: false,
            native_token: false,
        }
    }

    fn user_data(&self) -> *mut c_void {
        Arc::as_ptr(&self.slot).cast_mut().cast()
    }

    fn register(
        &mut self,
        callback: C,
        native_register: impl FnOnce(*mut c_void) -> c_int,
    ) -> MvsResult<()> {
        if self.registered {
            return Err(MvsError::CallOrder);
        }
        self.slot.set(callback);
        if !self.native_token {
            let native = Arc::into_raw(Arc::clone(&self.slot));
            debug_assert_eq!(native, Arc::as_ptr(&self.slot));
            self.native_token = true;
        }

        if let Err(error) = check(native_register(self.user_data())) {
            self.slot.clear();
            return Err(error);
        }

        self.registered = true;
        Ok(())
    }

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

    /// 回收注册时留给 SDK 的 raw Arc strong ref。
    ///
    /// # Safety
    ///
    /// 仅可在 DestroyHandle 成功后调用。
    unsafe fn release_native(&mut self) {
        if self.native_token {
            self.native_token = false;
            // SAFETY: native_token 精确对应一次 Arc::into_raw。
            unsafe { drop(Arc::from_raw(Arc::as_ptr(&self.slot))) };
        }
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
    fn new(raw: *mut c_void) -> Option<Self> {
        NonNull::new(raw).map(Self)
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// 已打开的 MVS 相机及其局部资源。
pub(crate) struct Camera {
    handle: Option<NativeHandle>,
    grabbing: bool,
    image_cb: Option<CallbackRecord<ImageCallbackFn>>,
    exception_cb: Option<CallbackRecord<ExceptionCallbackFn>>,
    event_cbs: Vec<EventRecord>,
}

impl Camera {
    /// 创建并打开 handle；打开失败时按厂商示例尽力回滚 DestroyHandle。
    pub(crate) fn open(
        device: DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> MvsResult<Self> {
        let mut raw_handle = std::ptr::null_mut();
        // SAFETY: 输出地址可写，device 是 Rust-owned SDK 结构体快照。
        let create = check(unsafe { sys::MV_CC_CreateHandle(&mut raw_handle, device.raw()) });
        if let Err(error) = create {
            if let Some(handle) = NativeHandle::new(raw_handle) {
                // SAFETY: SDK 写出的非空 handle 尚未转移给其它 owner。
                let _ = check(unsafe { sys::MV_CC_DestroyHandle(handle.as_ptr()) });
            }
            return Err(error);
        }
        let handle = NativeHandle::new(raw_handle).ok_or(MvsError::Handle)?;

        // SAFETY: handle 来自 CreateHandle，访问模式与切换 key 直接转发。
        let open =
            check(unsafe { sys::MV_CC_OpenDevice(handle.as_ptr(), mode.raw(), switchover_key) });
        if let Err(error) = open {
            // SAFETY: OpenDevice 失败后 handle 仍由本函数唯一持有。
            let _ = check(unsafe { sys::MV_CC_DestroyHandle(handle.as_ptr()) });
            return Err(error);
        }

        Ok(Self {
            handle: Some(handle),
            grabbing: false,
            image_cb: None,
            exception_cb: None,
            event_cbs: Vec::new(),
        })
    }

    fn handle(&self) -> MvsResult<*mut c_void> {
        self.handle
            .as_ref()
            .map(NativeHandle::as_ptr)
            .ok_or(MvsError::CallOrder)
    }

    fn stopped_handle(&self) -> MvsResult<*mut c_void> {
        if self.grabbing {
            return Err(MvsError::CallOrder);
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
        let handle = self.stopped_handle()?;

        // SAFETY: handle 已打开，且本地状态为 stopped。
        check(unsafe { sys::MV_CC_StartGrabbing(handle) })?;
        self.grabbing = true;
        Ok(())
    }

    /// 仅在 native 调用成功后更新本地状态，失败时允许原操作重试。
    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        if !self.grabbing {
            return Err(MvsError::CallOrder);
        }
        let handle = self.handle()?;
        // SAFETY: 本地状态记录当前正在取流。
        check(unsafe { sys::MV_CC_StopGrabbing(handle) })?;
        self.grabbing = false;
        Ok(())
    }

    /// polling 模式获取的 buffer 由 FrameGuard 唯一负责归还。
    pub(crate) fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        if !self.grabbing
            || self
                .image_cb
                .as_ref()
                .is_some_and(CallbackRecord::is_active)
        {
            return Err(MvsError::CallOrder);
        }
        let handle = self.handle()?;
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw 是可写输出结构体，handle 正在 polling 取流。
        check(unsafe { sys::MV_CC_GetImageBuffer(handle, &mut raw, timeout_ms) })?;
        Ok(FrameGuard::new(handle, raw))
    }

    /// image callback 的注册与注销都要求停止取流，匹配厂商示例顺序。
    pub(crate) fn register_image_callback(&mut self, callback: ImageCallbackFn) -> MvsResult<()> {
        let handle = self.stopped_handle()?;
        let record = self.image_cb.get_or_insert_with(CallbackRecord::new);
        record.register(callback, |user| {
            // SAFETY: native Arc token 使 slot 稳定；autoFree=true 限定 Frame 借用期。
            unsafe { sys::MV_CC_RegisterImageCallBackEx2(handle, Some(image_trampoline), user, 1) }
        })
    }

    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        let handle = self.stopped_handle()?;
        let Some(record) = self.image_cb.as_mut() else {
            return Ok(());
        };
        record.unregister(|| {
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

    pub(crate) fn get_string(&self, key: &str) -> MvsResult<String> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_STRINGVALUE::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetStringValue(handle, key.as_ptr(), &mut value) })?;
        let end = value
            .chCurValue
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(value.chCurValue.len());
        // SAFETY: Windows c_char 为 i8，只重解释已初始化字段的前 end 个字节。
        let bytes =
            unsafe { std::slice::from_raw_parts(value.chCurValue.as_ptr().cast::<u8>(), end) };
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    pub(crate) fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
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
        let handle = self.handle()?;
        let record = self.exception_cb.get_or_insert_with(CallbackRecord::new);
        record.register(callback, |user| {
            // SAFETY: slot 地址稳定并匹配 exception trampoline 类型。
            unsafe {
                sys::MV_CC_RegisterExceptionCallBack(handle, Some(exception_trampoline), user)
            }
        })
    }

    pub(crate) fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        let handle = self.handle()?;
        let Some(record) = self.exception_cb.as_mut() else {
            return Ok(());
        };
        record.unregister(|| {
            // SAFETY: CHM 说明 null callback/user 注销 exception callback。
            unsafe { sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut()) }
        })
    }

    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        callback: EventCallbackFn,
    ) -> MvsResult<()> {
        let handle = self.handle()?;
        let name = CString::new(event_name)?;
        let index = self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str())
            .unwrap_or_else(|| {
                self.event_cbs.push(EventRecord {
                    name,
                    callback: CallbackRecord::new(),
                });
                self.event_cbs.len() - 1
            });
        let name_ptr = self.event_cbs[index].name.as_ptr();
        self.event_cbs[index].callback.register(callback, |user| {
            // SAFETY: event name 和 slot 均由 EventRecord 稳定持有。
            unsafe {
                sys::MV_CC_RegisterEventCallBackEx(handle, name_ptr, Some(event_trampoline), user)
            }
        })
    }

    pub(crate) fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        let handle = self.handle()?;
        let name = CString::new(event_name)?;
        let Some(index) = self
            .event_cbs
            .iter()
            .position(|record| record.name.as_c_str() == name.as_c_str())
        else {
            return Ok(());
        };
        let name_ptr = self.event_cbs[index].name.as_ptr();
        self.event_cbs[index].callback.unregister(|| {
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

    pub(crate) fn debug_details(&self) -> (&'static str, Option<&'static str>, bool, bool, usize) {
        (
            if self.handle.is_some() {
                if self.grabbing { "Grabbing" } else { "Open" }
            } else {
                "Closed"
            },
            self.grabbing.then(|| {
                if self
                    .image_cb
                    .as_ref()
                    .is_some_and(CallbackRecord::is_active)
                {
                    "Callback"
                } else {
                    "Polling"
                }
            }),
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

    /// 尝试完整 teardown，只返回第一个错误；DestroyHandle 始终是最后一步。
    pub(crate) fn cleanup(&mut self) -> MvsResult<()> {
        if self.handle.is_none() {
            return Ok(());
        }
        self.silence_callbacks();
        let handle = self.handle.take().expect("handle checked above").as_ptr();
        let mut first_error = None;

        if self.grabbing {
            // SAFETY: handle 尚由本 Camera 持有；失败不阻止后续 teardown。
            record_result(
                &mut first_error,
                check(unsafe { sys::MV_CC_StopGrabbing(handle) }),
            );
        }
        if self
            .image_cb
            .as_ref()
            .is_some_and(|record| record.registered)
        {
            // SAFETY: 官方 Ex2 示例以 null callback/user 注销。
            record_result(
                &mut first_error,
                check(unsafe {
                    sys::MV_CC_RegisterImageCallBackEx2(handle, None, std::ptr::null_mut(), 1)
                }),
            );
        }
        if self
            .exception_cb
            .as_ref()
            .is_some_and(|record| record.registered)
        {
            // SAFETY: null callback/user 注销 exception callback。
            record_result(
                &mut first_error,
                check(unsafe {
                    sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut())
                }),
            );
        }
        for record in &self.event_cbs {
            if record.callback.registered {
                // SAFETY: event name 仍由 record 持有。
                record_result(
                    &mut first_error,
                    check(unsafe {
                        sys::MV_CC_RegisterEventCallBackEx(
                            handle,
                            record.name.as_ptr(),
                            None,
                            std::ptr::null_mut(),
                        )
                    }),
                );
            }
        }

        // SAFETY: teardown 继续遵循 CloseDevice → DestroyHandle。
        record_result(
            &mut first_error,
            check(unsafe { sys::MV_CC_CloseDevice(handle) }),
        );
        let destroyed = record_result(
            &mut first_error,
            check(unsafe { sys::MV_CC_DestroyHandle(handle) }),
        );

        self.grabbing = false;
        if destroyed {
            // SAFETY: DestroyHandle 成功后实例失效，SDK 不再访问 pUser。
            unsafe { self.release_callback_tokens() };
            self.image_cb = None;
            self.exception_cb = None;
            self.event_cbs.clear();
        }

        first_error.map_or(Ok(()), Err)
    }

    fn silence_callbacks(&self) {
        if let Some(record) = &self.image_cb {
            record.silence();
        }
        if let Some(record) = &self.exception_cb {
            record.silence();
        }
        for record in &self.event_cbs {
            record.callback.silence();
        }
    }

    /// 回收所有由 native 持有的 callback Arc token。
    ///
    /// # Safety
    ///
    /// 仅在 DestroyHandle 成功后调用。
    unsafe fn release_callback_tokens(&mut self) {
        if let Some(record) = &mut self.image_cb {
            // SAFETY: caller 已确认 DestroyHandle 成功。
            unsafe { record.release_native() };
        }
        if let Some(record) = &mut self.exception_cb {
            // SAFETY: caller 已确认 DestroyHandle 成功。
            unsafe { record.release_native() };
        }
        for record in &mut self.event_cbs {
            // SAFETY: caller 已确认 DestroyHandle 成功。
            unsafe { record.callback.release_native() };
        }
    }
}

impl Drop for Camera {
    /// native handle 与 callback slot 属于 backend Camera，Drop 负责局部兜底清理。
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn record_result(first_error: &mut Option<MvsError>, result: MvsResult<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some(error);
            }
            false
        }
    }
}
