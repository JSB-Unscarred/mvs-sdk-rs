//! 非 Windows x86_64 MSVC 目标的同形 backend；所有 native 入口返回平台错误。

use std::convert::Infallible;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::camera::{EventCallback, ExceptionCallback, ImageCallback};
use crate::device::DeviceProperties;
use crate::frame::{Frame, FrameInfo};
use crate::library::RuntimeCore;
use crate::text::SdkText;
use crate::{
    AccessMode, CleanupError, EnumValue, FloatValue, IntValue, MvsError, MvsResult, TransportLayer,
};

fn unsupported<T>() -> MvsResult<T> {
    Err(MvsError::UnsupportedPlatform)
}

pub(crate) struct Sdk;

impl Sdk {
    pub(crate) fn init() -> MvsResult<Self> {
        unsupported()
    }

    pub(crate) fn sdk_version() -> MvsResult<u32> {
        unsupported()
    }

    pub(crate) fn finalize(&self) -> MvsResult<()> {
        unsupported()
    }
}

pub(crate) fn enumerate_devices(_layers: TransportLayer) -> MvsResult<Vec<DeviceInfo>> {
    unsupported()
}

/// 枚举在该平台恒失败，因此本值不会被构造，`decode` 不再编造字段。
#[derive(Clone)]
pub(crate) struct DeviceInfo;

impl DeviceInfo {
    pub(crate) fn decode(&self) -> DeviceProperties {
        unreachable!("device enumeration is unavailable on this platform")
    }

    pub(crate) fn is_accessible(&self, _mode: AccessMode) -> bool {
        false
    }

    pub(crate) fn as_raw(&self) -> *const c_void {
        std::ptr::null()
    }
}

#[derive(Debug)]
pub(crate) struct Camera;

impl Camera {
    pub(crate) fn open(
        _runtime: Arc<RuntimeCore>,
        _device: DeviceInfo,
        _mode: AccessMode,
        _switchover_key: u16,
    ) -> MvsResult<Self> {
        unsupported()
    }

    pub(crate) fn as_raw_handle(&self) -> *mut c_void {
        std::ptr::null_mut()
    }

    pub(crate) fn is_connected(&self) -> bool {
        false
    }

    pub(crate) fn start_grabbing(&mut self) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn stop_grabbing(&mut self) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_image_buffer(&self, _timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        unsupported()
    }

    pub(crate) fn register_image_callback(&mut self, _callback: ImageCallback) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_int(&self, _key: &str) -> MvsResult<IntValue> {
        unsupported()
    }

    pub(crate) fn set_int(&self, _key: &str, _value: i64) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_enum(&self, _key: &str) -> MvsResult<EnumValue> {
        unsupported()
    }

    pub(crate) fn set_enum_value(&self, _key: &str, _value: u32) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn set_enum_symbolic(&self, _key: &str, _value: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_float(&self, _key: &str) -> MvsResult<FloatValue> {
        unsupported()
    }

    pub(crate) fn set_float(&self, _key: &str, _value: f32) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_bool(&self, _key: &str) -> MvsResult<bool> {
        unsupported()
    }

    pub(crate) fn set_bool(&self, _key: &str, _value: bool) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_string(&self, _key: &str) -> MvsResult<SdkText> {
        unsupported()
    }

    pub(crate) fn set_string(&self, _key: &str, _value: &[u8]) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn exec_command(&self, _key: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn register_exception_callback(
        &mut self,
        _callback: ExceptionCallback,
    ) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn unregister_exception_callback(&mut self) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn register_event_callback(
        &mut self,
        _event_name: &str,
        _callback: EventCallback,
    ) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn unregister_event_callback(&mut self, _event_name: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn event_notification_on(&self, _event_name: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn event_notification_off(&self, _event_name: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), CleanupError> {
        Ok(())
    }
}

/// 取图在该平台恒失败，因此该 guard 不可构造；`'cam` 仍需保留以匹配公开签名。
pub(crate) struct FrameGuard<'cam> {
    never: Infallible,
    _marker: PhantomData<&'cam ()>,
}

impl FrameGuard<'_> {
    pub(crate) fn frame(&self) -> Frame<'_> {
        match self.never {}
    }

    pub(crate) fn info(&self) -> FrameInfo {
        match self.never {}
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        match self.never {}
    }
}
