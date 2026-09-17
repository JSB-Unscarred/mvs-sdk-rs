//! 已打开的相机：handle 所有权、`GenICam` 节点读写与 exception/event callback。

use std::any::Any;
use std::ffi::{CStr, CString};
use std::fmt;
use std::mem;
use std::os::raw::c_void;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use crate::callback::{event_trampoline, exception_trampoline};
use crate::error::sdk_call;
use crate::sdk::Session;
use crate::{
    AccessMode, DeviceInfo, Error, ErrorCode, EventInfo, ExceptionKind, Result,
    fixed_cstr_from_chars, sys,
};

/// 交给 SDK 的 callback 闭包；只做类型擦除后的释放。
pub(crate) type BoxedCallback = Box<dyn Any + Send + Sync>;

/// 已打开的 MVS 相机。
///
/// `Camera` 独占 native handle，释放时依次调用 `MV_CC_CloseDevice` 与 `MV_CC_DestroyHandle`；
/// 需要观察清理错误时调用 [`Camera::close`]。相机持有 SDK 会话的引用，不借用
/// [`Sdk`](crate::Sdk)。`Camera` 是 `Send` 但不是 `Sync`，同一 handle 上的调用由 owner 串行发起。
pub struct Camera {
    /// 只在 `release` 中被取走，存活的相机总是持有 handle。
    handle: Option<NonNull<c_void>>,
    /// 已交给 SDK 的 exception/event 闭包；SDK 可能随时回调，只在 `DestroyHandle` 成功后释放。
    callbacks: Vec<BoxedCallback>,
    session: Arc<Session>,
}

// SAFETY: 厂商示例在工作线程中使用 handle。Camera 独占 handle 且不是 Sync，调用不会并发。
unsafe impl Send for Camera {}

/// Integer 节点的当前值与取值约束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntValue {
    /// 当前值。
    pub current: i64,
    /// 最小值。
    pub min: i64,
    /// 最大值。
    pub max: i64,
    /// 步长。
    pub inc: i64,
}

/// Float 节点的当前值与取值范围。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatValue {
    /// 当前值。
    pub current: f32,
    /// 最小值。
    pub min: f32,
    /// 最大值。
    pub max: f32,
}

/// Enumeration 节点的当前值与候选值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumValue {
    /// 当前值。
    pub current: u32,
    /// 节点支持的全部值。
    pub supported: Vec<u32>,
}

impl Camera {
    /// 创建并打开 handle；`OpenDevice` 失败时销毁 handle。
    pub(crate) fn open(
        session: Arc<Session>,
        device: &DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: handle 是可写输出；CreateHandle 在调用期间复制设备记录。
        unsafe { sdk_call!(MV_CC_CreateHandle(&raw mut handle, device.raw())) }?;
        let handle = NonNull::new(handle)
            .ok_or(Error::Sdk { function: "MV_CC_CreateHandle", code: ErrorCode::Handle })?;

        // SAFETY: handle 来自 CreateHandle，只由本函数持有。
        let opened =
            unsafe { sdk_call!(MV_CC_OpenDevice(handle.as_ptr(), mode as u32, switchover_key)) };
        if let Err(error) = opened {
            // SAFETY: OpenDevice 失败后 handle 仍只由本函数持有。
            if unsafe { sdk_call!(MV_CC_DestroyHandle(handle.as_ptr())) }.is_err() {
                // handle 未能销毁，保留会话引用以阻止 Finalize。
                mem::forget(session);
            }
            return Err(error);
        }

        Ok(Self { handle: Some(handle), callbacks: Vec::new(), session })
    }

    /// native handle，供尚未封装的 SDK 接口使用。
    ///
    /// 通过它改变取流、callback 注册或 handle 生命周期会破坏本 crate 的约定。
    pub fn as_raw_handle(&self) -> *mut c_void {
        self.handle.map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// 设备当前是否在线。
    pub fn is_connected(&self) -> bool {
        // SAFETY: handle 在相机存活期间有效。
        unsafe { sys::MV_CC_IsDeviceConnected(self.as_raw_handle()) != 0 }
    }

    /// 读取 Integer 节点。
    pub fn get_int(&self, key: &CStr) -> Result<IntValue> {
        let mut value = sys::MVCC_INTVALUE_EX::default();
        // SAFETY: key 以 NUL 结尾，value 是可写输出，二者只在本次调用期间借出。
        unsafe {
            sdk_call!(MV_CC_GetIntValueEx(self.as_raw_handle(), key.as_ptr(), &raw mut value))
        }?;
        Ok(IntValue { current: value.nCurValue, min: value.nMin, max: value.nMax, inc: value.nInc })
    }

    /// 设置 Integer 节点。
    pub fn set_int(&self, key: &CStr, value: i64) -> Result<()> {
        // SAFETY: key 以 NUL 结尾，只在本次调用期间借出。
        unsafe { sdk_call!(MV_CC_SetIntValueEx(self.as_raw_handle(), key.as_ptr(), value)) }
    }

    /// 读取 Enumeration 节点。
    pub fn get_enum(&self, key: &CStr) -> Result<EnumValue> {
        let mut value = sys::MVCC_ENUMVALUE_EX::default();
        // SAFETY: key 以 NUL 结尾，value 是可写输出。
        unsafe {
            sdk_call!(MV_CC_GetEnumValueEx(self.as_raw_handle(), key.as_ptr(), &raw mut value))
        }?;
        let count = (value.nSupportedNum as usize).min(value.nSupportValue.len());
        Ok(EnumValue { current: value.nCurValue, supported: value.nSupportValue[..count].to_vec() })
    }

    /// 按数值设置 Enumeration 节点。
    pub fn set_enum_value(&self, key: &CStr, value: u32) -> Result<()> {
        // SAFETY: key 以 NUL 结尾。
        unsafe { sdk_call!(MV_CC_SetEnumValue(self.as_raw_handle(), key.as_ptr(), value)) }
    }

    /// 按符号名设置 Enumeration 节点。
    pub fn set_enum_symbolic(&self, key: &CStr, value: &CStr) -> Result<()> {
        // SAFETY: 两个字符串都以 NUL 结尾。
        unsafe {
            sdk_call!(MV_CC_SetEnumValueByString(
                self.as_raw_handle(),
                key.as_ptr(),
                value.as_ptr()
            ))
        }
    }

    /// 读取 Float 节点。
    pub fn get_float(&self, key: &CStr) -> Result<FloatValue> {
        let mut value = sys::MVCC_FLOATVALUE::default();
        // SAFETY: key 以 NUL 结尾，value 是可写输出。
        unsafe {
            sdk_call!(MV_CC_GetFloatValue(self.as_raw_handle(), key.as_ptr(), &raw mut value))
        }?;
        Ok(FloatValue { current: value.fCurValue, min: value.fMin, max: value.fMax })
    }

    /// 设置 Float 节点。
    pub fn set_float(&self, key: &CStr, value: f32) -> Result<()> {
        // SAFETY: key 以 NUL 结尾。
        unsafe { sdk_call!(MV_CC_SetFloatValue(self.as_raw_handle(), key.as_ptr(), value)) }
    }

    /// 读取 Boolean 节点。
    pub fn get_bool(&self, key: &CStr) -> Result<bool> {
        let mut value = 0;
        // SAFETY: key 以 NUL 结尾，value 是可写输出。
        unsafe {
            sdk_call!(MV_CC_GetBoolValue(self.as_raw_handle(), key.as_ptr(), &raw mut value))
        }?;
        Ok(value != 0)
    }

    /// 设置 Boolean 节点。
    pub fn set_bool(&self, key: &CStr, value: bool) -> Result<()> {
        // SAFETY: key 以 NUL 结尾。
        unsafe {
            sdk_call!(MV_CC_SetBoolValue(
                self.as_raw_handle(),
                key.as_ptr(),
                sys::bool_::from(value)
            ))
        }
    }

    /// 读取 String 节点，保留 SDK 原始字节。
    pub fn get_string(&self, key: &CStr) -> Result<CString> {
        let mut value = sys::MVCC_STRINGVALUE::default();
        // SAFETY: key 以 NUL 结尾，value 是可写输出。
        unsafe {
            sdk_call!(MV_CC_GetStringValue(self.as_raw_handle(), key.as_ptr(), &raw mut value))
        }?;
        Ok(fixed_cstr_from_chars(&value.chCurValue).to_owned())
    }

    /// 设置 String 节点。
    pub fn set_string(&self, key: &CStr, value: &CStr) -> Result<()> {
        // SAFETY: 两个字符串都以 NUL 结尾。
        unsafe {
            sdk_call!(MV_CC_SetStringValue(self.as_raw_handle(), key.as_ptr(), value.as_ptr()))
        }
    }

    /// 执行 Command 节点。
    pub fn exec_command(&self, key: &CStr) -> Result<()> {
        // SAFETY: key 以 NUL 结尾。
        unsafe { sdk_call!(MV_CC_SetCommandValue(self.as_raw_handle(), key.as_ptr())) }
    }

    /// 注册设备 exception callback，替换之前的注册。
    ///
    /// SDK 在内部线程调用 `callback`；闭包保留到 `DestroyHandle`，因此重复注册会累积闭包。
    /// callback 内的 panic 会在 FFI 边界终止进程。
    pub fn register_exception_callback<F>(&mut self, callback: F) -> Result<()>
    where
        F: Fn(ExceptionKind) + Send + Sync + 'static,
    {
        let callback = Box::new(callback);
        let user = ptr::from_ref(callback.as_ref()).cast_mut().cast();
        // SAFETY: trampoline 与 F 匹配；闭包在 DestroyHandle 之前不会释放。
        unsafe {
            sdk_call!(MV_CC_RegisterExceptionCallBack(
                self.as_raw_handle(),
                Some(exception_trampoline::<F>),
                user
            ))
        }?;
        self.callbacks.push(callback);
        Ok(())
    }

    /// 注销 exception callback；已注册的闭包仍保留到 `DestroyHandle`。
    pub fn unregister_exception_callback(&mut self) -> Result<()> {
        // SAFETY: 厂商约定传入空 callback 注销。
        unsafe {
            sdk_call!(MV_CC_RegisterExceptionCallBack(self.as_raw_handle(), None, ptr::null_mut()))
        }
    }

    /// 为名为 `event_name` 的 `GenICam` 事件注册 callback，替换该事件之前的注册。
    ///
    /// 闭包的保留与 panic 规则同 [`Camera::register_exception_callback`]。设备端的事件开关见
    /// [`Camera::event_notification_on`]。
    pub fn register_event_callback<F>(&mut self, event_name: &CStr, callback: F) -> Result<()>
    where
        F: Fn(&EventInfo<'_>) + Send + Sync + 'static,
    {
        // 厂商未说明是否复制事件名，名字与闭包一起保留到 DestroyHandle。
        let name = event_name.to_owned();
        let callback = Box::new(callback);
        let user = ptr::from_ref(callback.as_ref()).cast_mut().cast();
        // SAFETY: name 以 NUL 结尾；trampoline 与 F 匹配，name 与闭包在 DestroyHandle 之前不会释放。
        unsafe {
            sdk_call!(MV_CC_RegisterEventCallBackEx(
                self.as_raw_handle(),
                name.as_ptr(),
                Some(event_trampoline::<F>),
                user
            ))
        }?;
        self.callbacks.push(callback);
        self.callbacks.push(Box::new(name));
        Ok(())
    }

    /// 注销名为 `event_name` 的事件 callback；已注册的闭包仍保留到 `DestroyHandle`。
    pub fn unregister_event_callback(&mut self, event_name: &CStr) -> Result<()> {
        // SAFETY: event_name 以 NUL 结尾；厂商约定传入空 callback 注销。
        unsafe {
            sdk_call!(MV_CC_RegisterEventCallBackEx(
                self.as_raw_handle(),
                event_name.as_ptr(),
                None,
                ptr::null_mut()
            ))
        }
    }

    /// 打开设备端的事件通知。
    pub fn event_notification_on(&self, event_name: &CStr) -> Result<()> {
        // SAFETY: event_name 以 NUL 结尾。
        unsafe { sdk_call!(MV_CC_EventNotificationOn(self.as_raw_handle(), event_name.as_ptr())) }
    }

    /// 关闭设备端的事件通知。
    pub fn event_notification_off(&self, event_name: &CStr) -> Result<()> {
        // SAFETY: event_name 以 NUL 结尾。
        unsafe { sdk_call!(MV_CC_EventNotificationOff(self.as_raw_handle(), event_name.as_ptr())) }
    }

    /// 关闭并销毁 handle，返回首个错误。
    pub fn close(mut self) -> Result<()> {
        self.release()
    }

    /// 交给 SDK 且可能仍被回调的闭包，保留到 `DestroyHandle` 之后。
    pub(crate) fn keep_until_destroy(&mut self, callback: BoxedCallback) {
        self.callbacks.push(callback);
    }

    /// 取走 handle 并依次 Close、Destroy；`close` 之后的 `Drop` 因此不会重复释放。
    ///
    /// `CloseDevice` 失败不影响 `DestroyHandle`。Destroy 失败时 SDK 可能仍持有闭包指针和会话资源，
    /// 因此泄漏闭包与一份会话引用，Finalize 不再执行。
    fn release(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let handle = handle.as_ptr();
        // SAFETY: handle 由相机独占；借用相机的取流守卫与 buffer 都已释放。
        let close = unsafe { sdk_call!(MV_CC_CloseDevice(handle)) };
        // SAFETY: 同上，且之后不再使用 handle。
        let destroy = unsafe { sdk_call!(MV_CC_DestroyHandle(handle)) };
        if destroy.is_err() {
            mem::forget(mem::take(&mut self.callbacks));
            mem::forget(Arc::clone(&self.session));
        }
        close.and(destroy)
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera")
            .field("handle", &self.as_raw_handle())
            .field("callbacks", &self.callbacks.len())
            .finish_non_exhaustive()
    }
}
