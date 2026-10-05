//! 取流状态：pull 与 callback 两种模式各对应一个借用相机的守卫。
//!
//! 守卫可变借用 [`Camera`]，因此取流期间无法再次开始取流、注册 callback 或关闭相机；
//! 节点读写经 `Deref` 仍可使用。守卫释放时停止取流，需要观察错误时调用 `stop`。

use std::fmt;
use std::mem;
use std::ops::Deref;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use crate::callback::{image_trampoline, into_user_data};
use crate::error::sdk_call;
use crate::{Camera, Frame, FrameGuard, Result, sys};

impl Camera {
    /// 开始主动取图，之后用 [`Grabbing::get_image_buffer`] 取图。
    pub fn start_grabbing(&mut self) -> Result<Grabbing<'_>> {
        // SAFETY: 可变借用保证当前没有其它取流守卫。
        unsafe { sdk_call!(MV_CC_StartGrabbing(self.as_raw_handle())) }?;
        Ok(Grabbing { camera: self })
    }

    /// 注册 image callback 并开始取流。
    ///
    /// `callback` 在 SDK 的线程中运行，其中的 panic 会终止进程；[`Frame`] 只在本次回调中有效。
    pub fn start_grabbing_with<F>(&mut self, callback: F) -> Result<CallbackGrabbing<'_>>
    where
        F: Fn(Frame<'_>) + Send + Sync + 'static,
    {
        let handle = self.as_raw_handle();
        let (callback, user) = into_user_data(callback);
        // SAFETY: trampoline 与 F 匹配；守卫持有闭包到 `StopGrabbing` 与注销都成功。
        unsafe {
            sdk_call!(MV_CC_RegisterImageCallBackEx2(
                handle,
                Some(image_trampoline::<F>),
                user,
                1
            ))
        }?;

        // SAFETY: handle 已注册 image callback，且没有其它取流守卫。
        if let Err(error) = unsafe { sdk_call!(MV_CC_StartGrabbing(handle)) } {
            if unregister_image_callback(self).is_err() {
                self.keep_until_destroy(callback);
            }
            return Err(error);
        }
        Ok(CallbackGrabbing {
            camera: self,
            callback: Some(callback),
        })
    }
}

/// 主动取图的取流守卫，释放时停止取流。
#[derive(Debug)]
#[must_use = "grabbing stops when the guard is dropped"]
pub struct Grabbing<'a> {
    camera: &'a mut Camera,
}

impl Grabbing<'_> {
    /// 等待一帧并借出 SDK buffer，`None` 表示无限等待；超时返回 [`ErrorCode::NoData`](crate::ErrorCode::NoData)。
    pub fn get_image_buffer(&self, timeout: Option<Duration>) -> Result<FrameGuard<'_>> {
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw 是可写输出，buffer 由返回的守卫归还。
        unsafe {
            sdk_call!(MV_CC_GetImageBuffer(
                self.camera.as_raw_handle(),
                &raw mut raw,
                timeout_ms(timeout)
            ))
        }?;
        Ok(FrameGuard::new(self.camera.as_raw_handle(), &raw))
    }

    /// 停止取流并返回 SDK 的结果。
    pub fn stop(self) -> Result<()> {
        let result = stop_grabbing(self.camera);
        mem::forget(self);
        result
    }
}

impl Deref for Grabbing<'_> {
    type Target = Camera;

    fn deref(&self) -> &Camera {
        self.camera
    }
}

impl Drop for Grabbing<'_> {
    fn drop(&mut self) {
        let _ = stop_grabbing(self.camera);
    }
}

/// callback 取图的取流守卫，释放时停止取流并注销 callback。
#[must_use = "grabbing stops and the callback is unregistered when the guard is dropped"]
pub struct CallbackGrabbing<'a> {
    camera: &'a mut Camera,
    /// 只在 `release` 中被取走，`stop` 之后的 `Drop` 因此不会重复停止。
    callback: Option<Arc<dyn Send + Sync>>,
}

impl CallbackGrabbing<'_> {
    /// 停止取流并注销 callback，返回第一个错误。
    pub fn stop(mut self) -> Result<()> {
        self.release()
    }

    /// 停止取流并注销 callback；`StopGrabbing` 或注销失败时 SDK 可能仍会回调，闭包转交相机保留到
    /// `DestroyHandle`。
    fn release(&mut self) -> Result<()> {
        let Some(callback) = self.callback.take() else {
            return Ok(());
        };
        let stop = stop_grabbing(self.camera);
        let unregister = unregister_image_callback(self.camera);
        if stop.is_err() || unregister.is_err() {
            self.camera.keep_until_destroy(callback);
        }
        stop.and(unregister)
    }
}

impl Deref for CallbackGrabbing<'_> {
    type Target = Camera;

    fn deref(&self) -> &Camera {
        self.camera
    }
}

impl Drop for CallbackGrabbing<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl fmt::Debug for CallbackGrabbing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackGrabbing")
            .field("camera", &self.camera)
            .finish_non_exhaustive()
    }
}

/// 停止取流；调用方是该相机唯一的取流守卫。
fn stop_grabbing(camera: &Camera) -> Result<()> {
    // SAFETY: 调用方是该相机唯一的取流守卫。
    unsafe { sdk_call!(MV_CC_StopGrabbing(camera.as_raw_handle())) }
}

/// 注销 image callback。
fn unregister_image_callback(camera: &Camera) -> Result<()> {
    // SAFETY: 官方 Ex2 示例以空 callback 与 user 注销，`bAutoFree` 仍传 true。
    unsafe {
        sdk_call!(MV_CC_RegisterImageCallBackEx2(
            camera.as_raw_handle(),
            None,
            ptr::null_mut(),
            1
        ))
    }
}

/// 转换为 SDK 的毫秒等待时间：`u32::MAX` 是无限等待，有限等待最多取 `u32::MAX - 1`；
/// 不足 1 毫秒的部分向上取整，与 std 在 Windows 上的超时换算一致，避免短等待退化为不等待。
fn timeout_ms(timeout: Option<Duration>) -> u32 {
    timeout.map_or(u32::MAX, |timeout| {
        u32::try_from(timeout.as_nanos().div_ceil(1_000_000))
            .map_or(u32::MAX - 1, |ms| ms.min(u32::MAX - 1))
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::timeout_ms;

    // 有限等待不会退化成 SDK 的无限等待哨兵，亚毫秒等待向上取整。
    #[test]
    fn finite_timeouts_never_become_infinite() {
        assert_eq!(timeout_ms(None), u32::MAX);
        assert_eq!(timeout_ms(Some(Duration::ZERO)), 0);
        assert_eq!(timeout_ms(Some(Duration::from_micros(1))), 1);
        assert_eq!(timeout_ms(Some(Duration::MAX)), u32::MAX - 1);
    }
}
