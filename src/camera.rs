//! 已打开相机的安全接口。

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::backend;
use crate::callback::EventInfo;
use crate::frame::{Frame, FrameGuard};
use crate::library::RuntimeCore;
use crate::text::SdkText;
use crate::{AccessMode, CleanupError, EnumValue, FloatValue, IntValue, MvsError, MvsResult};

pub(crate) type ImageCallback = Arc<dyn Fn(&Frame<'_>) + Send + Sync + 'static>;
pub(crate) type ExceptionCallback = Arc<dyn Fn(u32) + Send + Sync + 'static>;
pub(crate) type EventCallback = Arc<dyn Fn(&EventInfo<'_>) + Send + Sync + 'static>;

/// Windows `INFINITE`；有限等待 API 拒绝该哨兵，无限等待使用 blocking 方法。
const INFINITE_WAIT_MS: u32 = u32::MAX;

/// 已打开的 MVS 相机。
///
/// `Camera` 内部持有进程级 SDK session lease，因而不借用 [`crate::Sdk`]。
/// 它可以移动到普通 worker thread，但不实现 `Sync`；同一 handle 的调用由 owner
/// 串行发起。`Drop` 只做忽略错误的兜底，正常路径使用 [`Camera::close`]。
/// 取流与 callback 注册状态只在对应 native 调用返回 `MV_OK` 后更新；
/// native 失败保留调用前状态并返回原错误，本地顺序冲突返回
/// [`crate::MvsError::InvalidState`]。
pub struct Camera {
    inner: backend::Camera,
    _not_sync: PhantomData<Cell<()>>,
}

impl Camera {
    pub(crate) fn open(
        runtime: Arc<RuntimeCore>,
        device: backend::DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> MvsResult<Self> {
        Ok(Self {
            inner: backend::Camera::open(runtime, device, mode, switchover_key)?,
            _not_sync: PhantomData,
        })
    }

    /// 借出 opaque native handle，供尚未包装的 SDK 接口使用。
    ///
    /// # Safety
    ///
    /// pointer 由本 `Camera` 所有。通过 raw API 修改取流、callback 或 handle
    /// 生命周期会破坏 safe 层状态。
    pub unsafe fn as_raw_handle(&self) -> *mut c_void {
        self.inner.as_raw_handle()
    }

    /// 返回当前连接状态快照。
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// 注册 image callback，使用 `MV_CC_RegisterImageCallBackEx2(autoFree=true)`。
    ///
    /// 注册与注销要求停止取流。同一注册只接受一次；先注销后可重新注册。
    /// `Frame` 仅在本次调用期间有效，跨线程或长期使用时调用
    /// [`Frame::to_owned`]。callback 内 panic 在 FFI 边界终止进程。
    /// 业务错误应由 closure 通过 channel 通知 owner；它们不会成为本注册调用的
    /// `MvsResult`。当前线程位于 MVS callback 时，生命周期操作返回
    /// [`MvsError::InvalidState`]；`close` / `Drop` 则终止进程。
    ///
    /// callback 由 SDK thread 调用，因此 capture 必须 `Send + Sync`：
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use mvs_sdk_rs::Camera;
    ///
    /// fn register_non_send(camera: &mut Camera) {
    ///     let state = Rc::new(());
    ///     let _ = camera.register_image_callback(move |_| drop(Rc::clone(&state)));
    /// }
    /// ```
    pub fn register_image_callback<F>(&mut self, callback: F) -> MvsResult<()>
    where
        F: Fn(&Frame<'_>) + Send + Sync + 'static,
    {
        self.inner.register_image_callback(Arc::new(callback))
    }

    /// 注销 image callback。
    ///
    /// 返回后不再开始新的 Rust 调用；已经进入 trampoline 的调用可短暂继续，
    /// 其 closure 由独立 Arc 保活。
    pub fn unregister_image_callback(&mut self) -> MvsResult<()> {
        self.inner.unregister_image_callback()
    }

    /// 启动取流；已注册 image callback 时使用 callback 模式，否则使用 polling。
    pub fn start_grabbing(&mut self) -> MvsResult<()> {
        self.inner.start_grabbing()
    }

    /// 停止取流。
    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.inner.stop_grabbing()
    }

    /// polling 模式下获取一帧 SDK buffer，`timeout_ms` 为有限等待。
    ///
    /// `u32::MAX` 是 SDK 的无限等待哨兵，请改用 [`Self::get_image_buffer_blocking`]。
    /// guard 借用相机并在 [`FrameGuard::release`] 或 `Drop` 时归还 buffer。
    pub fn get_image_buffer(&self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        self.inner
            .get_image_buffer(finite_timeout_ms(timeout_ms)?)
            .map(FrameGuard::new)
    }

    /// polling 模式下无限等待一帧 SDK buffer。
    pub fn get_image_buffer_blocking(&self) -> MvsResult<FrameGuard<'_>> {
        self.inner
            .get_image_buffer(INFINITE_WAIT_MS)
            .map(FrameGuard::new)
    }

    /// polling 模式下获取并复制一帧，同时显式归还 SDK buffer。
    ///
    /// `u32::MAX` 请改用 [`Self::get_owned_frame_blocking`]。
    /// buffer release 失败会覆盖已完成的 owned copy 并返回对应错误，避免调用方误以为
    /// 本次 native buffer 已正常归还。
    pub fn get_owned_frame(&mut self, timeout_ms: u32) -> MvsResult<crate::OwnedFrame> {
        self.owned_frame_with_timeout(finite_timeout_ms(timeout_ms)?)
    }

    /// polling 模式下无限等待、复制一帧并显式归还 SDK buffer。
    pub fn get_owned_frame_blocking(&mut self) -> MvsResult<crate::OwnedFrame> {
        self.owned_frame_with_timeout(INFINITE_WAIT_MS)
    }

    fn owned_frame_with_timeout(&mut self, timeout_ms: u32) -> MvsResult<crate::OwnedFrame> {
        let frame = self
            .inner
            .get_image_buffer(timeout_ms)
            .map(FrameGuard::new)?;
        let owned = frame.to_owned();
        frame.release()?;
        Ok(owned)
    }

    /// 获取 Integer 节点当前值、范围和步长。
    pub fn get_int(&self, key: &str) -> MvsResult<IntValue> {
        self.inner.get_int(key)
    }

    /// 设置 Integer 节点。
    pub fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        self.inner.set_int(key, value)
    }

    /// 获取 Enum 节点当前值和支持值列表。
    pub fn get_enum(&self, key: &str) -> MvsResult<EnumValue> {
        self.inner.get_enum(key)
    }

    /// 按 numeric value 设置 Enum 节点。
    pub fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        self.inner.set_enum_value(key, value)
    }

    /// 按 symbolic name 设置 Enum 节点。
    pub fn set_enum_symbolic(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_enum_symbolic(key, value)
    }

    /// 获取 Float 节点当前值和范围。
    pub fn get_float(&self, key: &str) -> MvsResult<FloatValue> {
        self.inner.get_float(key)
    }

    /// 设置 Float 节点。
    pub fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        self.inner.set_float(key, value)
    }

    /// 获取 Boolean 节点。
    pub fn get_bool(&self, key: &str) -> MvsResult<bool> {
        self.inner.get_bool(key)
    }

    /// 设置 Boolean 节点。
    pub fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        self.inner.set_bool(key, value)
    }

    /// 获取 String 节点，保留 SDK 原始字节。
    pub fn get_string(&self, key: &str) -> MvsResult<SdkText> {
        self.inner.get_string(key)
    }

    /// 设置 String 节点；`value` 为原始字节，拒绝 interior NUL。
    pub fn set_string(&self, key: &str, value: &[u8]) -> MvsResult<()> {
        self.inner.set_string(key, value)
    }

    /// 执行 Command 节点。
    pub fn exec_command(&self, key: &str) -> MvsResult<()> {
        self.inner.exec_command(key)
    }

    /// 注册设备 exception callback。
    ///
    /// closure 只用于通知；需要关闭或重连时通过 channel 交给 Camera owner。
    pub fn register_exception_callback<F>(&mut self, callback: F) -> MvsResult<()>
    where
        F: Fn(u32) + Send + Sync + 'static,
    {
        self.inner.register_exception_callback(Arc::new(callback))
    }

    /// 注销 exception callback；已经进入的调用可短暂继续。
    pub fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        self.inner.unregister_exception_callback()
    }

    /// 注册一个 named GenICam event callback。
    pub fn register_event_callback<F>(&mut self, event_name: &str, callback: F) -> MvsResult<()>
    where
        F: Fn(&EventInfo<'_>) + Send + Sync + 'static,
    {
        self.inner
            .register_event_callback(event_name, Arc::new(callback))
    }

    /// 注销一个 named event callback；已经进入的调用可短暂继续。
    pub fn unregister_event_callback(&mut self, event_name: &str) -> MvsResult<()> {
        self.inner.unregister_event_callback(event_name)
    }

    /// 开启设备端 named event notification。
    pub fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_on(event_name)
    }

    /// 关闭设备端 named event notification。
    pub fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_off(event_name)
    }

    /// 消费相机并按 Stop → callback 注销 → Close → Destroy 顺序清理。
    ///
    /// 全部清理步骤只尝试一次；错误返回后不能使用同一 `Camera` 重试。
    /// [`CleanupError`] 保留首个 Destroy 前操作及错误，并独立保留 Destroy 错误。
    /// 当前线程位于 MVS callback 时终止进程。
    pub fn close(mut self) -> Result<(), CleanupError> {
        self.inner.cleanup()
    }
}

fn finite_timeout_ms(timeout_ms: u32) -> MvsResult<u32> {
    if timeout_ms == INFINITE_WAIT_MS {
        Err(MvsError::InvalidState(
            "u32::MAX selects infinite wait; call the blocking method instead",
        ))
    } else {
        Ok(timeout_ms)
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use super::{INFINITE_WAIT_MS, finite_timeout_ms};
    use crate::MvsError;

    // 有限等待 API 拒绝 SDK 无限等待哨兵，避免与 blocking 入口混淆。
    #[test]
    fn finite_timeout_rejects_infinite_sentinel() {
        assert!(matches!(
            finite_timeout_ms(INFINITE_WAIT_MS),
            Err(MvsError::InvalidState(_))
        ));
        assert_eq!(finite_timeout_ms(0).unwrap(), 0);
        assert_eq!(
            finite_timeout_ms(INFINITE_WAIT_MS - 1).unwrap(),
            u32::MAX - 1
        );
    }
}
