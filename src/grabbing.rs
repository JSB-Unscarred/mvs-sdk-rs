//! 取流状态：pull 与 callback 两种模式各对应一个守卫。
//!
//! 守卫对相机的持有方式泛型（[`HoldsCamera`]）：`&mut Camera` 借用相机，适合在一个作用域内取流；
//! `Camera` 按值持有，守卫可以存进结构体，`stop` 后交还相机。两种方式下取流期间都无法再次开始取流、
//! 注册 callback 或关闭相机；节点读写经 `Deref` 仍可使用。守卫释放时停止取流，需要观察错误时调用 `stop`。

use std::borrow::BorrowMut;
use std::fmt;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use crate::callback::{image_trampoline, into_user_data};
use crate::error::sdk_call;
use crate::{Camera, Error, Frame, FrameGuard, Result, sys};

/// 取流守卫持有相机的方式：`&mut Camera` 借用，`Camera` 按值持有。
///
/// sealed trait：守卫依赖每次借出的都是开始取流的同一台相机，任意 `BorrowMut` 实现无法保证这一点。
pub trait HoldsCamera: sealed::Sealed + BorrowMut<Camera> {}

impl HoldsCamera for Camera {}
impl HoldsCamera for &mut Camera {}

mod sealed {
    use crate::Camera;

    pub trait Sealed {}

    impl Sealed for Camera {}
    impl Sealed for &mut Camera {}
}

impl Camera {
    /// 借用相机开始主动取图，等同于 [`Grabbing::start`]；按值持有时直接调用后者。
    pub fn start_grabbing(&mut self) -> Result<Grabbing<&mut Self>> {
        Grabbing::start(self).map_err(|(_, error)| error)
    }

    /// 借用相机注册 image callback 并开始取流，等同于 [`CallbackGrabbing::start`]；按值持有时直接调用后者。
    pub fn start_grabbing_with<F>(&mut self, callback: F) -> Result<CallbackGrabbing<&mut Self>>
    where
        F: Fn(Frame<'_>) + Send + Sync + 'static,
    {
        CallbackGrabbing::start(self, callback).map_err(|(_, error)| error)
    }
}

/// 主动取图的取流守卫，释放时停止取流。
#[derive(Debug)]
#[must_use = "grabbing stops when the guard is dropped"]
pub struct Grabbing<C: HoldsCamera> {
    camera: C,
}

impl<C: HoldsCamera> Grabbing<C> {
    /// 开始主动取图，之后用 [`Grabbing::get_image_buffer`] 取图；失败时交还相机。
    pub fn start(camera: C) -> std::result::Result<Self, (C, Error)> {
        // SAFETY: 守卫独占相机，当前没有其它取流守卫。
        match unsafe { sdk_call!(MV_CC_StartGrabbing(camera.borrow().as_raw_handle())) } {
            Ok(()) => Ok(Self { camera }),
            Err(error) => Err((camera, error)),
        }
    }

    /// 等待一帧并借出 SDK buffer，`None` 表示无限等待；超时返回 [`ErrorCode::NoData`](crate::ErrorCode::NoData)。
    pub fn get_image_buffer(&self, timeout: Option<Duration>) -> Result<FrameGuard<'_>> {
        let handle = self.camera.borrow().as_raw_handle();
        let mut raw = sys::MV_FRAME_OUT::default();
        // SAFETY: raw 是可写输出，buffer 由返回的守卫归还。
        unsafe { sdk_call!(MV_CC_GetImageBuffer(handle, &raw mut raw, timeout_ms(timeout))) }?;
        Ok(FrameGuard::new(handle, &raw))
    }

    /// 停止取流，交还相机与 SDK 的结果。
    pub fn stop(self) -> (C, Result<()>) {
        let this = ManuallyDrop::new(self);
        let result = stop_grabbing(this.camera.borrow());
        // SAFETY: this 不再使用也不会 drop，camera 只读出这一次。
        (unsafe { ptr::read(&raw const this.camera) }, result)
    }
}

impl<C: HoldsCamera> Deref for Grabbing<C> {
    type Target = Camera;

    fn deref(&self) -> &Camera {
        self.camera.borrow()
    }
}

impl<C: HoldsCamera> Drop for Grabbing<C> {
    fn drop(&mut self) {
        let _ = stop_grabbing(self.camera.borrow());
    }
}

/// callback 取图的取流守卫，释放时停止取流并注销 callback。
#[must_use = "grabbing stops and the callback is unregistered when the guard is dropped"]
pub struct CallbackGrabbing<C: HoldsCamera> {
    camera: C,
    /// 只在 `release` 中被取走，`stop` 之后的 `Drop` 因此不会重复停止。
    callback: Option<Arc<dyn Send + Sync>>,
}

impl<C: HoldsCamera> CallbackGrabbing<C> {
    /// 注册 image callback 并开始取流；失败时交还相机。
    ///
    /// `callback` 在 SDK 的线程中运行，其中的 panic 会终止进程；[`Frame`] 只在本次回调中有效。
    pub fn start<F>(mut camera: C, callback: F) -> std::result::Result<Self, (C, Error)>
    where
        F: Fn(Frame<'_>) + Send + Sync + 'static,
    {
        let handle = camera.borrow().as_raw_handle();
        let (callback, user) = into_user_data(callback);
        // SAFETY: trampoline 与 F 匹配；守卫持有闭包到 `StopGrabbing` 与注销都成功。
        let registered = unsafe {
            sdk_call!(MV_CC_RegisterImageCallBackEx2(
                handle,
                Some(image_trampoline::<F>),
                user,
                1
            ))
        };
        if let Err(error) = registered {
            return Err((camera, error));
        }

        // SAFETY: handle 已注册 image callback，且没有其它取流守卫。
        if let Err(error) = unsafe { sdk_call!(MV_CC_StartGrabbing(handle)) } {
            if unregister_image_callback(camera.borrow()).is_err() {
                camera.borrow_mut().keep_until_destroy(callback);
            }
            return Err((camera, error));
        }
        Ok(Self {
            camera,
            callback: Some(callback),
        })
    }

    /// 停止取流并注销 callback，交还相机与第一个错误。
    pub fn stop(self) -> (C, Result<()>) {
        let mut this = ManuallyDrop::new(self);
        let result = this.release();
        // SAFETY: this 不再使用也不会 drop（callback 已被 release 取走），camera 只读出这一次。
        (unsafe { ptr::read(&raw const this.camera) }, result)
    }

    /// 停止取流并注销 callback；`StopGrabbing` 或注销失败时 SDK 可能仍会回调，闭包转交相机保留到
    /// `DestroyHandle`。
    fn release(&mut self) -> Result<()> {
        let Some(callback) = self.callback.take() else {
            return Ok(());
        };
        let stop = stop_grabbing(self.camera.borrow());
        let unregister = unregister_image_callback(self.camera.borrow());
        if stop.is_err() || unregister.is_err() {
            self.camera.borrow_mut().keep_until_destroy(callback);
        }
        stop.and(unregister)
    }
}

impl<C: HoldsCamera> Deref for CallbackGrabbing<C> {
    type Target = Camera;

    fn deref(&self) -> &Camera {
        self.camera.borrow()
    }
}

impl<C: HoldsCamera> Drop for CallbackGrabbing<C> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl<C: HoldsCamera> fmt::Debug for CallbackGrabbing<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackGrabbing")
            .field("camera", self.camera.borrow())
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
