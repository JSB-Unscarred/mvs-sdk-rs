use std::marker::PhantomData;
use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::rc::Rc;

use crate::camera::{EventCallback, ExceptionCallback, ImageCallback};
use crate::frame::{Frame, FrameInfo, FrameMetadata};
use crate::{AccessMode, EnumNode, FloatNode, IntNode, MvsError, MvsResult, TransportLayer};

fn unsupported<T>() -> MvsResult<T> {
    Err(MvsError::UnsupportedPlatform)
}

pub(crate) struct Sdk;

impl Sdk {
    pub(crate) fn init() -> MvsResult<Self> {
        unsupported()
    }

    pub(crate) fn sdk_version(&self) -> u32 {
        0
    }
}

pub(crate) struct DeviceList;

impl DeviceList {
    pub(crate) fn enumerate(_layers: TransportLayer) -> MvsResult<Self> {
        unsupported()
    }

    pub(crate) fn len(&self) -> usize {
        0
    }

    pub(crate) fn get(&self, _index: usize) -> Option<DeviceInfo<'_>> {
        None
    }
}

#[derive(Copy, Clone)]
pub(crate) struct DeviceInfo<'a> {
    _marker: PhantomData<&'a ()>,
}

impl DeviceInfo<'_> {
    pub(crate) fn transport_layer(&self) -> TransportLayer {
        TransportLayer::UNKNOWN
    }

    pub(crate) fn is_gige(&self) -> bool {
        false
    }

    pub(crate) fn is_usb(&self) -> bool {
        false
    }

    pub(crate) fn manufacturer(&self) -> String {
        String::new()
    }

    pub(crate) fn model(&self) -> String {
        String::new()
    }

    pub(crate) fn serial(&self) -> String {
        String::new()
    }

    pub(crate) fn user_defined_name(&self) -> String {
        String::new()
    }

    pub(crate) fn ip(&self) -> Option<Ipv4Addr> {
        None
    }

    pub(crate) fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        None
    }

    pub(crate) fn is_accessible(&self, _mode: AccessMode) -> bool {
        false
    }

    pub(crate) fn as_raw(&self) -> *const c_void {
        std::ptr::null()
    }
}

pub(crate) struct Camera;

impl Camera {
    pub(crate) fn open(_device: DeviceInfo<'_>, _mode: AccessMode) -> MvsResult<Self> {
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

    pub(crate) fn get_image_buffer(&mut self, _timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        unsupported()
    }

    pub(crate) fn register_image_callback(&mut self, _callback: ImageCallback) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn unregister_image_callback(&mut self) -> MvsResult<()> {
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

    pub(crate) fn set_int(&self, _key: &str, _value: i64) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_int(&self, _key: &str) -> MvsResult<i64> {
        unsupported()
    }

    pub(crate) fn get_int_range(&self, _key: &str) -> MvsResult<IntNode> {
        unsupported()
    }

    pub(crate) fn set_float(&self, _key: &str, _value: f32) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_float(&self, _key: &str) -> MvsResult<f32> {
        unsupported()
    }

    pub(crate) fn get_float_range(&self, _key: &str) -> MvsResult<FloatNode> {
        unsupported()
    }

    pub(crate) fn set_bool(&self, _key: &str, _value: bool) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_bool(&self, _key: &str) -> MvsResult<bool> {
        unsupported()
    }

    pub(crate) fn set_enum(&self, _key: &str, _value: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn set_string(&self, _key: &str, _value: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn exec_command(&self, _key: &str) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn get_string(&self, _key: &str) -> MvsResult<String> {
        unsupported()
    }

    pub(crate) fn get_enum(&self, _key: &str) -> MvsResult<u32> {
        unsupported()
    }

    pub(crate) fn get_enum_info(&self, _key: &str) -> MvsResult<EnumNode> {
        unsupported()
    }

    pub(crate) fn set_enum_value(&self, _key: &str, _value: u32) -> MvsResult<()> {
        unsupported()
    }

    pub(crate) fn debug_details(&self) -> (&'static str, bool, bool, bool, usize) {
        ("Closed", false, false, false, 0)
    }

    pub(crate) fn close(&mut self) {}
}

pub(crate) struct FrameGuard<'cam> {
    metadata: FrameMetadata,
    _marker: PhantomData<&'cam mut ()>,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl FrameGuard<'_> {
    pub(crate) fn frame(&self) -> Frame<'_> {
        Frame::from_parts(&[], &self.metadata)
    }

    pub(crate) fn info(&self) -> FrameInfo<'_> {
        FrameInfo::from_metadata(&self.metadata)
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        Ok(())
    }
}
