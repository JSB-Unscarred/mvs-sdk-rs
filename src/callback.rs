//! SDK callback 的 trampoline 与回调参数类型。
//!
//! 注册时把 `Box<F>` 的地址作为 `pUser` 交给 SDK，trampoline 按注册时的具体类型 `F` 还原闭包，
//! 不需要锁或全局表。闭包由相机或取流守卫持有，释放时机见 [`crate::docs::architecture`]。
//! Rust 1.81 起，panic 越过 `extern "C"` 函数会直接终止进程。

use std::ffi::CStr;
use std::os::raw::{c_uint, c_void};

use crate::{Frame, fixed_cstr_from_chars, high_low, sys};

/// exception callback 收到的消息类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExceptionKind {
    /// 设备断开连接（`MV_EXCEPTION_DEV_DISCONNECT`）。
    Disconnected,
    /// SDK 与驱动版本不匹配（`MV_EXCEPTION_VERSION_CHECK`）。
    VersionCheck,
    /// 头文件未定义的消息类型。
    Other(u32),
}

impl ExceptionKind {
    const fn from_raw(raw: u32) -> Self {
        match raw {
            sys::MV_EXCEPTION_DEV_DISCONNECT => Self::Disconnected,
            sys::MV_EXCEPTION_VERSION_CHECK => Self::VersionCheck,
            other => Self::Other(other),
        }
    }
}

/// event callback 收到的事件信息，只在本次回调期间有效。
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EventInfo<'a> {
    /// 事件名。
    pub name: &'a CStr,
    /// 事件号。
    pub event_id: u16,
    /// 流通道号。
    pub stream_channel: u16,
    /// 帧号。
    pub block_id: u64,
    /// 设备时间戳，单位由厂商定义。
    pub timestamp: u64,
}

/// image callback 的 trampoline。
///
/// # Safety
///
/// `user` 必须是注册时传入的 `Box<F>` 地址且闭包仍然存活；`frame` 来自 `autoFree=true` 的注册，
/// 像素只在本次调用期间有效。
pub(crate) unsafe extern "C" fn image_trampoline<F>(
    frame: *mut sys::MV_FRAME_OUT,
    user: *mut c_void,
    _auto_free: sys::bool_,
) where
    F: Fn(&Frame<'_>),
{
    // SAFETY: 见函数的 Safety 约定。
    let (callback, frame) = unsafe { (&*user.cast::<F>(), frame.as_ref()) };
    if let Some(frame) = frame {
        // SAFETY: SDK 保证 buffer 在本次回调返回前有效。
        callback(&unsafe { Frame::from_raw(frame) });
    }
}

/// exception callback 的 trampoline。
///
/// # Safety
///
/// `user` 必须是注册时传入的 `Box<F>` 地址且闭包仍然存活。
pub(crate) unsafe extern "C" fn exception_trampoline<F>(msg_type: c_uint, user: *mut c_void)
where
    F: Fn(ExceptionKind),
{
    // SAFETY: 见函数的 Safety 约定。
    let callback = unsafe { &*user.cast::<F>() };
    callback(ExceptionKind::from_raw(msg_type));
}

/// event callback 的 trampoline。
///
/// # Safety
///
/// `user` 必须是注册时传入的 `Box<F>` 地址且闭包仍然存活；`info` 只在本次调用期间有效。
pub(crate) unsafe extern "C" fn event_trampoline<F>(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) where
    F: Fn(&EventInfo<'_>),
{
    // SAFETY: 见函数的 Safety 约定。
    let (callback, info) = unsafe { (&*user.cast::<F>(), info.as_ref()) };
    if let Some(info) = info {
        callback(&EventInfo {
            name: fixed_cstr_from_chars(&info.EventName),
            event_id: info.nEventID,
            stream_channel: info.nStreamChannel,
            block_id: high_low(info.nBlockIdHigh, info.nBlockIdLow),
            timestamp: high_low(info.nTimestampHigh, info.nTimestampLow),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;
    use std::ptr;
    use std::sync::Mutex;

    use super::{EventInfo, ExceptionKind, event_trampoline, exception_trampoline};
    use crate::sys;

    // 与注册时相同：闭包地址作 pUser，trampoline 由闭包类型实例化。
    unsafe fn fire_event<F: Fn(&EventInfo<'_>)>(callback: &F, info: &mut sys::MV_EVENT_OUT_INFO) {
        let user = ptr::from_ref(callback).cast_mut().cast::<c_void>();
        // SAFETY: 调用方保证闭包与 info 在同步调用期间有效。
        unsafe { event_trampoline::<F>(info, user) };
    }

    unsafe fn fire_exception<F: Fn(ExceptionKind)>(callback: &F, msg_type: u32) {
        let user = ptr::from_ref(callback).cast_mut().cast::<c_void>();
        // SAFETY: 调用方保证闭包在同步调用期间有效。
        unsafe { exception_trampoline::<F>(msg_type, user) };
    }

    // trampoline 按注册类型还原闭包，并转换事件名、时间戳与消息类型。
    #[test]
    fn trampolines_restore_the_closure_and_convert_arguments() {
        let seen = Mutex::new(Vec::new());
        let mut info = sys::MV_EVENT_OUT_INFO {
            nTimestampHigh: 0x1,
            nTimestampLow: 0x2,
            ..Default::default()
        };
        for (target, byte) in info.EventName.iter_mut().zip(b"End") {
            *target = byte.cast_signed();
        }

        // SAFETY: 闭包与 info 都是本函数的局部变量。
        unsafe {
            fire_event(
                &|event: &EventInfo<'_>| {
                    seen.lock().unwrap().push(format!("{:?}@{:X}", event.name, event.timestamp));
                },
                &mut info,
            );
            fire_exception(
                &|kind| seen.lock().unwrap().push(format!("{kind:?}")),
                sys::MV_EXCEPTION_DEV_DISCONNECT,
            );
        }

        assert_eq!(*seen.lock().unwrap(), ["\"End\"@100000002", "Disconnected"]);
    }
}
