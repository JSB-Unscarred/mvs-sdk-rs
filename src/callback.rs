//! SDK callback 的 trampoline 与回调参数类型。
//!
//! 闭包放在 `Arc` 中，`pUser` 是 `Arc::into_raw` 的地址，trampoline 按注册时的类型 `F` 还原闭包。
//! 相机或取流守卫持有一份强引用；trampoline 每次调用期间再持有一份，所以在 callback 里释放相机
//! 也不会释放正在执行的闭包。前提是 `StopGrabbing`、注销与 `DestroyHandle` 成功后 SDK 不再回调。
//! panic 越过 `extern "C"` 函数时进程终止（Rust 1.81 起）。

use std::ffi::CStr;
use std::ffi::{c_uint, c_void};
use std::sync::Arc;

use crate::{Frame, fixed_cstr, high_low, sys};

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
    /// 由 SDK 原始值构造，未定义的值保存在 [`ExceptionKind::Other`]。
    const fn from_raw(raw: u32) -> Self {
        match raw {
            sys::MV_EXCEPTION_DEV_DISCONNECT => Self::Disconnected,
            sys::MV_EXCEPTION_VERSION_CHECK => Self::VersionCheck,
            other => Self::Other(other),
        }
    }
}

/// event callback 收到的事件信息，只在本次回调期间有效。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    pub device_timestamp: u64,
}

/// 把闭包放入 `Arc`，返回 owner 持有的强引用与交给 SDK 的 `pUser`。
///
/// owner 只在 SDK 不再以该 `pUser` 回调后释放这份引用，清理失败时泄漏它。
pub(crate) fn into_user_data<F>(callback: F) -> (Arc<dyn Send + Sync>, *mut c_void)
where
    F: Send + Sync + 'static,
{
    let user = Arc::into_raw(Arc::new(callback));
    // SAFETY: user 刚由 into_raw 返回，取回的强引用交给 owner，SDK 只拿到地址。
    (unsafe { Arc::from_raw(user) }, user.cast_mut().cast())
}

/// 取回闭包，并在本次回调期间增持一份强引用：闭包在执行中释放 owner（例如 drop 相机）时，
/// 仍存活到调用返回。
///
/// # Safety
///
/// `user` 来自 [`into_user_data::<F>`]，且调用时 owner 的强引用尚未释放。
unsafe fn from_user_data<F>(user: *mut c_void) -> Arc<F> {
    let user = user.cast_const().cast::<F>();
    // SAFETY: 见函数的 Safety 约定。
    unsafe {
        Arc::increment_strong_count(user);
        Arc::from_raw(user)
    }
}

/// image callback 的 trampoline。
///
/// # Safety
///
/// `user` 来自 [`into_user_data::<F>`]，SDK 在 `StopGrabbing` 与注销都成功或 `DestroyHandle` 成功后
/// 不再以它回调；`frame` 来自 `bAutoFree = true` 的注册，像素只在本次调用期间有效。
pub(crate) unsafe extern "C" fn image_trampoline<F>(
    frame: *mut sys::MV_FRAME_OUT,
    user: *mut c_void,
    _auto_free: sys::bool_,
) where
    F: Fn(Frame<'_>),
{
    // SAFETY: 见函数的 Safety 约定。
    let (callback, frame) = unsafe { (from_user_data::<F>(user), frame.as_ref()) };
    if let Some(frame) = frame {
        // SAFETY: SDK 保证 buffer 在本次回调返回前有效。
        callback(unsafe { Frame::from_raw(frame) });
    }
}

/// exception callback 的 trampoline。
///
/// # Safety
///
/// `user` 来自 [`into_user_data::<F>`]，SDK 在 `DestroyHandle` 成功后不再以它回调。
pub(crate) unsafe extern "C" fn exception_trampoline<F>(msg_type: c_uint, user: *mut c_void)
where
    F: Fn(ExceptionKind),
{
    // SAFETY: 见函数的 Safety 约定。
    let callback = unsafe { from_user_data::<F>(user) };
    callback(ExceptionKind::from_raw(msg_type));
}

/// event callback 的 trampoline。
///
/// # Safety
///
/// 同 [`exception_trampoline`]；`info` 只在本次调用期间有效。
pub(crate) unsafe extern "C" fn event_trampoline<F>(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) where
    F: Fn(EventInfo<'_>),
{
    // SAFETY: 见函数的 Safety 约定。
    let (callback, info) = unsafe { (from_user_data::<F>(user), info.as_ref()) };
    if let Some(info) = info {
        callback(EventInfo {
            name: fixed_cstr(&info.EventName),
            event_id: info.nEventID,
            stream_channel: info.nStreamChannel,
            block_id: high_low(info.nBlockIdHigh, info.nBlockIdLow),
            device_timestamp: high_low(info.nTimestampHigh, info.nTimestampLow),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{ExceptionKind, exception_trampoline, into_user_data};

    static OWNER: Mutex<Option<Arc<dyn Send + Sync>>> = Mutex::new(None);

    // callback 在执行中释放 owner（相当于在回调里 drop 相机）时，闭包存活到本次调用返回，之后才释放。
    #[test]
    fn trampoline_keeps_the_closure_alive_for_the_call() {
        static ALIVE_AFTER_RELEASE: AtomicBool = AtomicBool::new(false);
        let captured = Arc::new(());
        let witness = Arc::clone(&captured);
        fire(move |_| {
            drop(OWNER.lock().unwrap().take());
            ALIVE_AFTER_RELEASE.store(Arc::strong_count(&captured) == 2, Ordering::SeqCst);
        });
        assert!(ALIVE_AFTER_RELEASE.load(Ordering::SeqCst));
        assert_eq!(Arc::strong_count(&witness), 1);
    }

    /// 与注册时相同地交出闭包并由 OWNER 持有，再像 SDK 一样调用一次 trampoline。
    fn fire<F: Fn(ExceptionKind) + Send + Sync + 'static>(callback: F) {
        let (owner, user) = into_user_data(callback);
        *OWNER.lock().unwrap() = Some(owner);
        // SAFETY: user 来自 into_user_data::<F>，调用开始时 owner 仍在 OWNER 中。
        unsafe { exception_trampoline::<F>(0, user) };
    }
}
