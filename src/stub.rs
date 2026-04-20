//! Stub types for non-Windows platforms.
//!
//! These types mirror the public API surface so that `cargo check` succeeds on
//! any host. Every method body is `unimplemented!()` — the stubs are never
//! executed, they only satisfy the type checker.

use std::fmt;
use std::marker::PhantomData;
use std::net::Ipv4Addr;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;
use std::time::Duration;

// ── Error ────────────────────────────────────────────────────────────

pub type MvsResult<T> = Result<T, MvsError>;

#[derive(Debug)]
pub enum MvsError {
    Handle,
    NotSupported,
    NoData,
    Parameter,
    Resource,
    Unknown(u32),
    Stub(String),
}

impl fmt::Display for MvsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for MvsError {}

impl MvsError {
    pub fn raw_code(&self) -> Option<u32> {
        None
    }
}

// ── Sdk ──────────────────────────────────────────────────────────────

pub struct Sdk {
    _private: (),
}

impl Sdk {
    pub fn init() -> MvsResult<Arc<Self>> {
        unimplemented!("MVS SDK is only available on Windows")
    }

    pub fn sdk_version(&self) -> u32 {
        unimplemented!()
    }

    pub fn enumerate_devices(self: &Arc<Self>, _layers: TransportLayer) -> MvsResult<DeviceList> {
        unimplemented!()
    }
}

// ── TransportLayer ───────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct TransportLayer(u32);

impl TransportLayer {
    pub const UNKNOWN: Self = Self(0);
    pub const GIGE: Self = Self(1);
    pub const USB: Self = Self(4);
    pub const CAMERALINK: Self = Self(8);
    pub const VIR_GIGE: Self = Self(0x10);
    pub const VIR_USB: Self = Self(0x20);
    pub const GENTL_GIGE: Self = Self(0x40);
    pub const GENTL_CAMERALINK: Self = Self(0x80);
    pub const GENTL_CXP: Self = Self(0x100);
    pub const GENTL_XOF: Self = Self(0x200);
    pub const GENTL_VIR: Self = Self(0x400);
    pub const ALL: Self = Self(0xFFFF);

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for TransportLayer {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for TransportLayer {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for TransportLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransportLayer(0x{:04X})", self.0)
    }
}

// ── DeviceList / DeviceIter / DeviceInfo ─────────────────────────────

pub struct DeviceList {
    _private: (),
}

impl DeviceList {
    pub fn len(&self) -> usize {
        0
    }
    pub fn is_empty(&self) -> bool {
        true
    }
    pub fn iter(&self) -> DeviceIter<'_> {
        DeviceIter {
            _marker: PhantomData,
        }
    }
    pub fn get(&self, _index: usize) -> Option<DeviceInfo<'_>> {
        None
    }
}

impl fmt::Debug for DeviceList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceList").finish()
    }
}

unsafe impl Send for DeviceList {}
unsafe impl Sync for DeviceList {}

pub struct DeviceIter<'a> {
    _marker: PhantomData<&'a ()>,
}

impl<'a> Iterator for DeviceIter<'a> {
    type Item = DeviceInfo<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

impl ExactSizeIterator for DeviceIter<'_> {}

#[derive(Copy, Clone)]
pub struct DeviceInfo<'a> {
    _marker: PhantomData<&'a ()>,
}

impl<'a> DeviceInfo<'a> {
    pub fn transport_layer(&self) -> TransportLayer {
        unimplemented!()
    }
    pub fn is_gige(&self) -> bool {
        unimplemented!()
    }
    pub fn is_usb(&self) -> bool {
        unimplemented!()
    }
    pub fn manufacturer(&self) -> String {
        unimplemented!()
    }
    pub fn model(&self) -> String {
        unimplemented!()
    }
    pub fn serial(&self) -> String {
        unimplemented!()
    }
    pub fn user_defined_name(&self) -> String {
        unimplemented!()
    }
    pub fn ip(&self) -> Option<Ipv4Addr> {
        unimplemented!()
    }
    pub fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        unimplemented!()
    }
    pub fn is_accessible(&self, _mode: AccessMode) -> bool {
        unimplemented!()
    }
    pub fn open(&self, _mode: AccessMode) -> MvsResult<Camera> {
        unimplemented!()
    }
    pub fn open_exclusive(&self) -> MvsResult<Camera> {
        unimplemented!()
    }
    pub fn open_control(&self) -> MvsResult<Camera> {
        unimplemented!()
    }
}

impl fmt::Debug for DeviceInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInfo").finish()
    }
}

// ── AccessMode ───────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AccessMode {
    Exclusive,
    ExclusiveWithSwitch,
    Control,
    ControlWithSwitch,
    ControlSwitchEnable,
    ControlSwitchEnableWithKey,
    Monitor,
}

// ── Camera ───────────────────────────────────────────────────────────

pub struct Camera {
    _private: (),
}

unsafe impl Send for Camera {}

impl Camera {
    pub fn is_connected(&self) -> bool {
        unimplemented!()
    }
    pub fn start_grabbing(&mut self) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn stop_grabbing(&mut self) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn get_image_buffer(&mut self, _timeout_ms: u32) -> MvsResult<FrameGuard<'_>> {
        unimplemented!()
    }
    pub fn register_image_callback<F>(&mut self, _f: F) -> MvsResult<()>
    where
        F: FnMut(&Frame<'_>) + Send + 'static,
    {
        unimplemented!()
    }
    pub fn unregister_image_callback(&mut self) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn register_exception_callback<F>(&mut self, _f: F) -> MvsResult<()>
    where
        F: FnMut(u32) + Send + 'static,
    {
        unimplemented!()
    }
    pub fn register_event_callback<F>(&mut self, _event_name: &str, _f: F) -> MvsResult<()>
    where
        F: FnMut(&EventInfo<'_>) + Send + 'static,
    {
        unimplemented!()
    }
    pub fn set_int(&self, _key: &str, _value: i64) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn get_int(&self, _key: &str) -> MvsResult<i64> {
        unimplemented!()
    }
    pub fn get_int_range(&self, _key: &str) -> MvsResult<IntNode> {
        unimplemented!()
    }
    pub fn set_float(&self, _key: &str, _value: f32) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn get_float(&self, _key: &str) -> MvsResult<f32> {
        unimplemented!()
    }
    pub fn get_float_range(&self, _key: &str) -> MvsResult<FloatNode> {
        unimplemented!()
    }
    pub fn set_bool(&self, _key: &str, _value: bool) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn get_bool(&self, _key: &str) -> MvsResult<bool> {
        unimplemented!()
    }
    pub fn set_enum(&self, _key: &str, _value: &str) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn set_string(&self, _key: &str, _value: &str) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn exec_command(&self, _key: &str) -> MvsResult<()> {
        unimplemented!()
    }
    pub fn get_string(&self, _key: &str) -> MvsResult<String> {
        unimplemented!()
    }
    pub fn get_enum(&self, _key: &str) -> MvsResult<u32> {
        unimplemented!()
    }
    pub fn get_enum_info(&self, _key: &str) -> MvsResult<EnumNode> {
        unimplemented!()
    }
    pub fn set_enum_value(&self, _key: &str, _value: u32) -> MvsResult<()> {
        unimplemented!()
    }
}

impl fmt::Debug for Camera {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Camera").finish()
    }
}

#[derive(Copy, Clone, Debug)]
pub struct IntNode {
    pub current: i64,
    pub min: i64,
    pub max: i64,
    pub inc: i64,
}

#[derive(Copy, Clone, Debug)]
pub struct FloatNode {
    pub current: f32,
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug)]
pub struct EnumNode {
    pub current: u32,
    pub supported: Vec<u32>,
}

// ── EventInfo ────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct EventInfo<'a> {
    _marker: PhantomData<&'a ()>,
}

impl<'a> EventInfo<'a> {
    pub fn name(&self) -> std::borrow::Cow<'_, str> {
        unimplemented!()
    }
    pub fn event_id(&self) -> u16 {
        unimplemented!()
    }
    pub fn stream_channel(&self) -> u16 {
        unimplemented!()
    }
    pub fn block_id(&self) -> u64 {
        unimplemented!()
    }
    pub fn timestamp(&self) -> u64 {
        unimplemented!()
    }
}

impl fmt::Debug for EventInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventInfo").finish()
    }
}

// ── Frame / FrameInfo / PixelType / OwnedFrame / FrameGuard ─────────

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PixelType(u32);

impl PixelType {
    pub const UNDEFINED: Self = Self(0);
    pub const MONO8: Self = Self(0x01080001);
    pub const RGB8_PACKED: Self = Self(0x02180014);
    pub const BGR8_PACKED: Self = Self(0x02180015);

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn bits_per_pixel(self) -> u32 {
        (self.0 >> 16) & 0xFF
    }
    pub const fn is_mono(self) -> bool {
        (self.0 & 0xFF000000) == 0x01000000
    }
    pub const fn is_color(self) -> bool {
        (self.0 & 0xFF000000) == 0x02000000
    }
    pub const fn is_custom(self) -> bool {
        (self.0 & 0x80000000) != 0
    }
}

impl fmt::Debug for PixelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PixelType(0x{:08X})", self.0)
    }
}

#[derive(Copy, Clone)]
pub struct FrameInfo<'a> {
    _marker: PhantomData<&'a ()>,
}

impl<'a> FrameInfo<'a> {
    pub fn width(&self) -> u32 {
        unimplemented!()
    }
    pub fn height(&self) -> u32 {
        unimplemented!()
    }
    pub fn pixel_type(&self) -> PixelType {
        unimplemented!()
    }
    pub fn frame_num(&self) -> u32 {
        unimplemented!()
    }
    pub fn frame_len(&self) -> u32 {
        unimplemented!()
    }
    pub fn offset_x(&self) -> u32 {
        unimplemented!()
    }
    pub fn offset_y(&self) -> u32 {
        unimplemented!()
    }
    pub fn gain(&self) -> f32 {
        unimplemented!()
    }
    pub fn exposure_time(&self) -> f32 {
        unimplemented!()
    }
    pub fn trigger_index(&self) -> u32 {
        unimplemented!()
    }
    pub fn lost_packets(&self) -> u32 {
        unimplemented!()
    }
    pub fn device_timestamp(&self) -> u64 {
        unimplemented!()
    }
    pub fn host_timestamp_raw(&self) -> i64 {
        unimplemented!()
    }
    pub fn host_timestamp(&self) -> Duration {
        unimplemented!()
    }
}

impl fmt::Debug for FrameInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameInfo").finish()
    }
}

pub struct Frame<'a> {
    _marker: PhantomData<&'a ()>,
}

impl<'a> Frame<'a> {
    pub fn data(&self) -> &[u8] {
        unimplemented!()
    }
    pub fn info(&self) -> &FrameInfo<'a> {
        unimplemented!()
    }
    pub fn to_owned(&self) -> OwnedFrame {
        unimplemented!()
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame").finish()
    }
}

#[derive(Clone)]
pub struct OwnedFrame {
    pub data: Vec<u8>,
}

impl OwnedFrame {
    pub fn info(&self) -> FrameInfo<'_> {
        unimplemented!()
    }
    pub fn as_frame(&self) -> Frame<'_> {
        unimplemented!()
    }
}

impl fmt::Debug for OwnedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedFrame").finish()
    }
}

pub struct FrameGuard<'cam> {
    _marker: PhantomData<&'cam ()>,
}

impl<'cam> FrameGuard<'cam> {
    pub fn frame(&self) -> Frame<'_> {
        unimplemented!()
    }
    pub fn info(&self) -> FrameInfo<'_> {
        unimplemented!()
    }
    pub fn to_owned(&self) -> OwnedFrame {
        unimplemented!()
    }
    pub fn release(self) -> MvsResult<()> {
        unimplemented!()
    }
}
