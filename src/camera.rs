//! Opened camera — the central public resource type.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::backend;
use crate::callback::EventInfo;
use crate::frame::{Frame, FrameGuard};
use crate::library::Sdk;
use crate::{AccessMode, EnumNode, FloatNode, IntNode, MvsResult};

pub(crate) type ImageCallback = Box<dyn FnMut(&Frame<'_>) + Send + 'static>;
pub(crate) type ExceptionCallback = Box<dyn FnMut(u32) + Send + 'static>;
pub(crate) type EventCallback = Box<dyn FnMut(&EventInfo<'_>) + Send + 'static>;

/// An opened MVS camera. `Send` but not `Sync`; concurrent access to one
/// camera requires external synchronization.
pub struct Camera {
    inner: backend::Camera,
    _library: Arc<Sdk>,
    _not_sync: PhantomData<Cell<()>>,
}

// SAFETY: the native SDK permits moving a handle between threads. Public
// methods require `&mut self` for stateful operations, and `Cell` keeps !Sync.
unsafe impl Send for Camera {}

impl Camera {
    pub(crate) fn open(
        device: backend::DeviceInfo<'_>,
        library: &Arc<Sdk>,
        mode: AccessMode,
    ) -> MvsResult<Self> {
        Ok(Self {
            inner: backend::Camera::open(device, mode)?,
            _library: Arc::clone(library),
            _not_sync: PhantomData,
        })
    }

    pub fn as_raw_handle(&self) -> *mut c_void {
        self.inner.as_raw_handle()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    pub fn start_grabbing(&mut self) -> MvsResult<()> {
        self.inner.start_grabbing()
    }

    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        self.inner.stop_grabbing()
    }

    pub fn get_image_buffer(&mut self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        self.inner.get_image_buffer(timeout_ms).map(FrameGuard::new)
    }

    pub fn register_image_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(&Frame<'_>) + Send + 'static,
    {
        self.inner.register_image_callback(Box::new(f))
    }

    pub fn unregister_image_callback(&mut self) -> MvsResult<()> {
        self.inner.unregister_image_callback()
    }

    pub fn register_exception_callback<F>(&mut self, f: F) -> MvsResult<()>
    where
        F: FnMut(u32) + Send + 'static,
    {
        self.inner.register_exception_callback(Box::new(f))
    }

    pub fn register_event_callback<F>(&mut self, event_name: &str, f: F) -> MvsResult<()>
    where
        F: FnMut(&EventInfo<'_>) + Send + 'static,
    {
        self.inner.register_event_callback(event_name, Box::new(f))
    }

    pub fn event_notification_on(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_on(event_name)
    }

    pub fn event_notification_off(&self, event_name: &str) -> MvsResult<()> {
        self.inner.event_notification_off(event_name)
    }

    pub fn set_int(&self, key: &str, value: i64) -> MvsResult<()> {
        self.inner.set_int(key, value)
    }

    pub fn get_int(&self, key: &str) -> MvsResult<i64> {
        self.inner.get_int(key)
    }

    pub fn get_int_range(&self, key: &str) -> MvsResult<IntNode> {
        self.inner.get_int_range(key)
    }

    pub fn set_float(&self, key: &str, value: f32) -> MvsResult<()> {
        self.inner.set_float(key, value)
    }

    pub fn get_float(&self, key: &str) -> MvsResult<f32> {
        self.inner.get_float(key)
    }

    pub fn get_float_range(&self, key: &str) -> MvsResult<FloatNode> {
        self.inner.get_float_range(key)
    }

    pub fn set_bool(&self, key: &str, value: bool) -> MvsResult<()> {
        self.inner.set_bool(key, value)
    }

    pub fn get_bool(&self, key: &str) -> MvsResult<bool> {
        self.inner.get_bool(key)
    }

    pub fn set_enum(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_enum(key, value)
    }

    pub fn set_string(&self, key: &str, value: &str) -> MvsResult<()> {
        self.inner.set_string(key, value)
    }

    pub fn exec_command(&self, key: &str) -> MvsResult<()> {
        self.inner.exec_command(key)
    }

    pub fn get_string(&self, key: &str) -> MvsResult<String> {
        self.inner.get_string(key)
    }

    pub fn get_enum(&self, key: &str) -> MvsResult<u32> {
        self.inner.get_enum(key)
    }

    pub fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode> {
        self.inner.get_enum_info(key)
    }

    pub fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()> {
        self.inner.set_enum_value(key, value)
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (grabbing, image_cb, exception_cb, event_cbs) = self.inner.debug_details();
        f.debug_struct("Camera")
            .field("handle", &self.as_raw_handle())
            .field("grabbing", &grabbing)
            .field("image_cb", &image_cb)
            .field("exception_cb", &exception_cb)
            .field("event_cbs", &event_cbs)
            .finish()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.inner.close();
    }
}
