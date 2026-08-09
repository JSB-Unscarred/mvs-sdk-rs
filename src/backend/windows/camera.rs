//! Windows 相机 handle、采集状态与节点访问。

use std::ffi::CString;
use std::os::raw::{c_float, c_int, c_void};
use std::ptr::NonNull;

use crate::camera::{
    EventCallback as EventCallbackFn, ExceptionCallback as ExceptionCallbackFn,
    ImageCallback as ImageCallbackFn,
};
use crate::error::check;
use crate::sys;
use crate::{AccessMode, EnumNode, FloatNode, IntNode, MvsError, MvsResult};

use super::callback::{
    CallbackSlot, drop_callback_safely, event_trampoline, exception_trampoline, image_trampoline,
};
use super::{DeviceInfo, FrameGuard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionMode {
    Callback,
    Polling,
}

impl AcquisitionMode {
    fn name(self) -> &'static str {
        match self {
            Self::Callback => "Callback",
            Self::Polling => "Polling",
        }
    }
}

impl AccessMode {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Exclusive => sys::MV_ACCESS_Exclusive,
            Self::ExclusiveWithSwitch => sys::MV_ACCESS_ExclusiveWithSwitch,
            Self::Control => sys::MV_ACCESS_Control,
            Self::ControlWithSwitch => sys::MV_ACCESS_ControlWithSwitch,
            Self::ControlSwitchEnable => sys::MV_ACCESS_ControlSwitchEnable,
            Self::ControlSwitchEnableWithKey(_) => sys::MV_ACCESS_ControlSwitchEnableWithKey,
            Self::Monitor => sys::MV_ACCESS_Monitor,
        }
    }

    pub(crate) const fn switchover_key(self) -> u16 {
        match self {
            Self::ControlSwitchEnableWithKey(key) => key,
            _ => 0,
        }
    }
}

/// callback slot 在 Camera 内唯一持有，Box 使传给 SDK 的 `pUser` 地址稳定。
struct CallbackRecord<C> {
    slot: Box<CallbackSlot<C>>,
    registered: bool,
}

impl<C> CallbackRecord<C> {
    fn new() -> Self {
        Self {
            slot: Box::new(CallbackSlot::new()),
            registered: false,
        }
    }

    fn user_data(&self) -> *mut c_void {
        (&*self.slot as *const CallbackSlot<C>).cast_mut().cast()
    }

    fn register(
        &mut self,
        callback: C,
        native_register: impl FnOnce(*mut c_void) -> c_int,
    ) -> MvsResult<()> {
        if let Some(previous) = self.slot.activate(callback) {
            drop_callback_safely(previous);
        }
        if self.registered {
            return Ok(());
        }

        if let Err(error) = check(native_register(self.user_data())) {
            if let Some(callback) = self.slot.deactivate() {
                drop_callback_safely(callback);
            }
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
        if let Some(callback) = self.slot.deactivate() {
            drop_callback_safely(callback);
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.registered && self.slot.is_active()
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
    grabbing: Option<AcquisitionMode>,
    image_cb: Option<CallbackRecord<ImageCallbackFn>>,
    exception_cb: Option<CallbackRecord<ExceptionCallbackFn>>,
    event_cbs: Vec<EventRecord>,
}

impl Camera {
    /// 创建并打开 handle；打开失败时按厂商示例尽力回滚 DestroyHandle。
    pub(crate) fn open(device: DeviceInfo, mode: AccessMode) -> MvsResult<Self> {
        let mut raw_handle = std::ptr::null_mut();
        // SAFETY: 输出地址可写，device 是 Rust-owned SDK 结构体快照。
        let create = check(unsafe { sys::MV_CC_CreateHandle(&mut raw_handle, device.raw()) });
        if let Err(error) = create {
            if let Some(handle) = NativeHandle::new(raw_handle) {
                // SAFETY: SDK 写出的非空 handle 尚未转移给其它 owner。
                if check(unsafe { sys::MV_CC_DestroyHandle(handle.as_ptr()) }).is_err() {
                    crate::library::retain_unresolved_handle();
                }
            }
            return Err(error);
        }
        let handle = NativeHandle::new(raw_handle).ok_or(MvsError::Handle)?;

        // SAFETY: handle 来自 CreateHandle，访问模式与切换 key 直接转发。
        let open = check(unsafe {
            sys::MV_CC_OpenDevice(handle.as_ptr(), mode.raw(), mode.switchover_key())
        });
        if let Err(error) = open {
            // SAFETY: OpenDevice 失败后 handle 仍由本函数唯一持有。
            if check(unsafe { sys::MV_CC_DestroyHandle(handle.as_ptr()) }).is_err() {
                crate::library::retain_unresolved_handle();
            }
            return Err(error);
        }

        Ok(Self {
            handle: Some(handle),
            grabbing: None,
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
        if self.grabbing.is_some() {
            return Err(MvsError::CallOrder);
        }
        self.handle()
    }

    fn handle_and_key(&self, key: &str) -> MvsResult<(*mut c_void, CString)> {
        Ok((self.handle()?, CString::new(key)?))
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

    /// 按 image callback 是否注册选择 callback 或 polling 模式。
    pub(crate) fn start_grabbing(&mut self) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.stopped_handle()?;
        let mode = match self.image_cb.as_ref() {
            Some(record) if record.is_active() => AcquisitionMode::Callback,
            Some(record) if record.registered => return Err(MvsError::CallOrder),
            _ => AcquisitionMode::Polling,
        };

        // SAFETY: handle 已打开，且本地状态为 stopped。
        check(unsafe { sys::MV_CC_StartGrabbing(handle) })?;
        self.grabbing = Some(mode);
        Ok(())
    }

    /// 仅在 native 调用成功后更新本地状态，失败时允许原操作重试。
    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.reject_callback_context()?;
        if self.grabbing.is_none() {
            return Err(MvsError::CallOrder);
        }
        let handle = self.handle()?;
        // SAFETY: 本地状态记录当前正在取流。
        check(unsafe { sys::MV_CC_StopGrabbing(handle) })?;
        self.grabbing = None;
        Ok(())
    }

    /// polling 模式获取的 buffer 由 FrameGuard 唯一负责归还。
    pub(crate) fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        if self.grabbing != Some(AcquisitionMode::Polling) {
            return Err(MvsError::CallOrder);
        }
        let handle = self.handle()?;
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw 是可写输出结构体，handle 正在 polling 取流。
        check(unsafe { sys::MV_CC_GetImageBuffer(handle, &mut raw, timeout_ms) })?;
        FrameGuard::new(handle, raw)
    }

    /// image callback 的注册、替换与注销都要求停止取流，匹配厂商示例顺序。
    pub(crate) fn register_image_callback(&mut self, callback: ImageCallbackFn) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.stopped_handle()?;
        let record = self.image_cb.get_or_insert_with(CallbackRecord::new);
        record.register(callback, |user| {
            // SAFETY: slot 地址稳定并由 Camera 持有到 handle teardown。
            unsafe { sys::MV_CC_RegisterImageCallBackEx(handle, Some(image_trampoline), user) }
        })
    }

    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        self.reject_callback_context()?;
        let handle = self.stopped_handle()?;
        let Some(record) = self.image_cb.as_mut() else {
            return Ok(());
        };
        record.unregister(|| {
            // SAFETY: null callback/user 是厂商示例使用的注销形式。
            unsafe { sys::MV_CC_RegisterImageCallBackEx(handle, None, std::ptr::null_mut()) }
        })
    }

    pub(crate) fn register_exception_callback(
        &mut self,
        callback: ExceptionCallbackFn,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
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
        self.reject_callback_context()?;
        let handle = self.handle()?;
        let Some(record) = self.exception_cb.as_mut() else {
            return Ok(());
        };
        record.unregister(|| {
            // SAFETY: null callback/user 注销当前 exception callback。
            unsafe { sys::MV_CC_RegisterExceptionCallBack(handle, None, std::ptr::null_mut()) }
        })
    }

    pub(crate) fn register_event_callback(
        &mut self,
        event_name: &str,
        callback: EventCallbackFn,
    ) -> MvsResult<()> {
        self.reject_callback_context()?;
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
        self.reject_callback_context()?;
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
            // SAFETY: name_ptr 在调用期间有效，null callback/user 表示注销。
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

    pub(crate) fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetIntValueEx(handle, key.as_ptr(), value) })
    }

    pub(crate) fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.get_int_range(key).map(|value| value.current)
    }

    pub(crate) fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_INTVALUE_EX::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetIntValueEx(handle, key.as_ptr(), &mut value) })?;
        Ok(IntNode {
            current: value.nCurValue,
            min: value.nMin,
            max: value.nMax,
            inc: value.nInc,
        })
    }

    pub(crate) fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetFloatValue(handle, key.as_ptr(), value as c_float) })
    }

    pub(crate) fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.get_float_range(key).map(|value| value.current)
    }

    pub(crate) fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_FLOATVALUE::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetFloatValue(handle, key.as_ptr(), &mut value) })?;
        Ok(FloatNode {
            current: value.fCurValue,
            min: value.fMin,
            max: value.fMax,
        })
    }

    pub(crate) fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        let value: sys::bool_ = if value { 1 } else { 0 };
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetBoolValue(handle, key.as_ptr(), value) })
    }

    pub(crate) fn get_bool(&self, key: &str) -> MvsResult<bool> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value: sys::bool_ = 0;
        // SAFETY: value 是可写输出参数。
        check(unsafe { sys::MV_CC_GetBoolValue(handle, key.as_ptr(), &mut value) })?;
        Ok(value != 0)
    }

    pub(crate) fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        let value = CString::new(value)?;
        // SAFETY: 两个字符串在调用期间有效。
        check(unsafe { sys::MV_CC_SetEnumValueByString(handle, key.as_ptr(), value.as_ptr()) })
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

    pub(crate) fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.get_enum_info(key).map(|value| value.current)
    }

    /// 使用 Ex 接口复制 SDK 最多 256 个候选值。
    pub(crate) fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
        let (handle, key) = self.handle_and_key(key)?;
        let mut value = sys::MVCC_ENUMVALUE_EX::default();
        // SAFETY: value 是可写输出结构体。
        check(unsafe { sys::MV_CC_GetEnumValueEx(handle, key.as_ptr(), &mut value) })?;
        let len = (value.nSupportedNum as usize).min(value.nSupportValue.len());
        Ok(EnumNode {
            current: value.nCurValue,
            supported: value.nSupportValue[..len].to_vec(),
        })
    }

    pub(crate) fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        let (handle, key) = self.handle_and_key(key)?;
        // SAFETY: key 在调用期间有效。
        check(unsafe { sys::MV_CC_SetEnumValue(handle, key.as_ptr(), value) })
    }

    pub(crate) fn debug_details(&self) -> (&'static str, Option<&'static str>, bool, bool, usize) {
        (
            if self.handle.is_some() {
                if self.grabbing.is_some() {
                    "Grabbing"
                } else {
                    "Open"
                }
            } else {
                "Closed"
            },
            self.grabbing.map(AcquisitionMode::name),
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
        if self.is_callback_context() {
            return self.abandon_from_callback();
        }

        self.stop_accepting_callbacks();
        let handle = self.handle.take().expect("handle checked above").as_ptr();
        let mut first_error = None;

        if self.grabbing.is_some() {
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
            // SAFETY: null callback/user 注销 image callback。
            record_result(
                &mut first_error,
                check(unsafe {
                    sys::MV_CC_RegisterImageCallBackEx(handle, None, std::ptr::null_mut())
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

        // 注销后等待已经进入 Rust closure 的 callback 返回。
        self.drain_callbacks();

        // SAFETY: teardown 继续遵循 CloseDevice → DestroyHandle。
        record_result(
            &mut first_error,
            check(unsafe { sys::MV_CC_CloseDevice(handle) }),
        );
        let destroyed = record_result(
            &mut first_error,
            check(unsafe { sys::MV_CC_DestroyHandle(handle) }),
        );

        self.grabbing = None;
        if destroyed {
            self.image_cb = None;
            self.exception_cb = None;
            self.event_cbs.clear();
        } else {
            // DestroyHandle 失败时 SDK 仍可能保存 pUser；仅泄漏这些稳定 slot。
            crate::library::retain_unresolved_handle();
            self.leak_callback_backing();
        }

        first_error.map_or(Ok(()), Err)
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
        if let Some(record) = &self.image_cb {
            drain_callback(record);
        }
        if let Some(record) = &self.exception_cb {
            drain_callback(record);
        }
        for record in &self.event_cbs {
            drain_callback(&record.callback);
        }
    }

    /// callback 内消费自身 Camera 属于误用；停用 closure 后泄漏 native backing 兜底。
    fn abandon_from_callback(&mut self) -> MvsResult<()> {
        self.stop_accepting_callbacks();
        crate::library::retain_unresolved_handle();
        let _ = self.handle.take();
        self.grabbing = None;
        self.leak_callback_backing();
        Err(MvsError::CallOrder)
    }

    fn leak_callback_backing(&mut self) {
        std::mem::forget(self.image_cb.take());
        std::mem::forget(self.exception_cb.take());
        std::mem::forget(std::mem::take(&mut self.event_cbs));
    }
}

impl Drop for Camera {
    /// native handle 与 callback slot 属于 backend Camera，Drop 负责局部兜底清理。
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn drain_callback<C>(record: &CallbackRecord<C>) {
    if let Some(callback) = record.slot.deactivate() {
        drop_callback_safely(callback);
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
