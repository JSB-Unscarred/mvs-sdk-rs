//! SDK callback 的 trampoline 与回调参数类型。
//!
//! 注册时把闭包放入 `Arc`，以 `Arc::into_raw` 的地址作为 `pUser` 交给 SDK，trampoline 按注册时的
//! 具体类型 `F` 还原闭包，不需要锁或全局表。相机或取流守卫持有一份强引用，释放时机见
//! [`crate::docs::architecture`]；trampoline 在每次调用期间再持有一份，callback 中释放相机时，
//! 正在执行的闭包因此存活到调用返回。Rust 1.81 起，panic 越过 `extern "C"` 函数会直接终止进程。

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
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        EventInfo, ExceptionKind, event_trampoline, exception_trampoline, image_trampoline,
        into_user_data,
    };
    use crate::{Frame, sys};

    // 与注册时相同：返回 owner 的强引用、交给 SDK 的 callback 与 pUser。
    fn register_image<F>(
        callback: F,
    ) -> (Arc<dyn Send + Sync>, sys::MvImageCallbackEx2, *mut c_void)
    where
        F: Fn(Frame<'_>) + Send + Sync + 'static,
    {
        let (owner, user) = into_user_data(callback);
        (owner, Some(image_trampoline::<F>), user)
    }

    fn register_event<F>(callback: F) -> (Arc<dyn Send + Sync>, sys::MvEventCallback, *mut c_void)
    where
        F: Fn(EventInfo<'_>) + Send + Sync + 'static,
    {
        let (owner, user) = into_user_data(callback);
        (owner, Some(event_trampoline::<F>), user)
    }

    fn register_exception<F>(
        callback: F,
    ) -> (Arc<dyn Send + Sync>, sys::MvExceptionCallback, *mut c_void)
    where
        F: Fn(ExceptionKind) + Send + Sync + 'static,
    {
        let (owner, user) = into_user_data(callback);
        (owner, Some(exception_trampoline::<F>), user)
    }

    // trampoline 按注册类型还原闭包并转换参数。
    #[test]
    fn trampolines_restore_the_closure_and_convert_arguments() {
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut pixels = [1_u8, 2];
        let mut frame = sys::MV_FRAME_OUT {
            pBufAddr: pixels.as_mut_ptr(),
            stFrameInfo: sys::MV_FRAME_OUT_INFO_EX {
                nFrameLen: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut info = sys::MV_EVENT_OUT_INFO {
            nTimestampHigh: 0x1,
            nTimestampLow: 0x2,
            ..Default::default()
        };
        for (target, byte) in info.EventName.iter_mut().zip(b"End") {
            *target = byte.cast_signed();
        }
        let (_image_owner, on_image, image_user) =
            register_image(|frame| SEEN.lock().unwrap().push(format!("{:?}", frame.data)));
        let (_event_owner, on_event, event_user) = register_event(|event| {
            let seen = format!("{:?}@{:X}", event.name, event.device_timestamp);
            SEEN.lock().unwrap().push(seen);
        });
        let (_exception_owner, on_exception, exception_user) =
            register_exception(|kind| SEEN.lock().unwrap().push(format!("{kind:?}")));

        // SAFETY: owner 在同步调用期间存活，frame、pixels 与 info 是本函数的局部变量。
        unsafe {
            on_image.unwrap()(&raw mut frame, image_user, 1);
            on_event.unwrap()(&raw mut info, event_user);
            on_exception.unwrap()(sys::MV_EXCEPTION_DEV_DISCONNECT, exception_user);
        }

        assert_eq!(
            *SEEN.lock().unwrap(),
            ["[1, 2]", "\"End\"@100000002", "Disconnected"]
        );
    }

    // callback 在执行中释放 owner（相当于在回调里 drop 相机）时，闭包存活到本次调用返回。
    #[test]
    fn trampoline_keeps_the_closure_alive_for_the_call() {
        static OWNER: Mutex<Option<Arc<dyn Send + Sync>>> = Mutex::new(None);
        static DROPPED: AtomicBool = AtomicBool::new(false);
        static ALIVE_AFTER_RELEASE: AtomicBool = AtomicBool::new(false);

        // 闭包捕获的探针，释放时置位 DROPPED。
        struct Probe(&'static AtomicBool);
        impl Probe {
            fn alive(&self) -> bool {
                !self.0.load(Ordering::SeqCst)
            }
        }
        impl Drop for Probe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let probe = Probe(&DROPPED);
        let (owner, on_exception, user) = register_exception(move |_| {
            drop(OWNER.lock().unwrap().take());
            ALIVE_AFTER_RELEASE.store(probe.alive(), Ordering::SeqCst);
        });
        *OWNER.lock().unwrap() = Some(owner);
        // SAFETY: 调用开始时 owner 仍在 OWNER 中。
        unsafe { on_exception.unwrap()(0, user) };

        assert!(ALIVE_AFTER_RELEASE.load(Ordering::SeqCst));
        assert!(DROPPED.load(Ordering::SeqCst));
    }
}
