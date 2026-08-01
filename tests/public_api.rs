//! Compile-time contract for the API visible to downstream crates.
//!
//! Cargo compiles integration tests as separate crates, so this file cannot
//! use private implementation details. It intentionally contains no runtime
//! test functions: all assertions are evaluated while compiling this crate,
//! so its test harness can link on Windows without pulling native MVS symbols.

#![allow(dead_code)]

use std::borrow::Cow;
use std::cell::Cell;
use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::sync::Arc;
use std::time::Duration;

use mvs_sdk_rs::error::{
    CleanupError as ModuleCleanupError, CleanupFailure as ModuleCleanupFailure,
    CleanupStep as ModuleCleanupStep, MvsError as ModuleError, MvsResult as ModuleResult,
};
use mvs_sdk_rs::{
    AccessMode, Camera, CleanupError, CleanupFailure, CleanupStep, DeviceInfo, DeviceIter,
    DeviceList, EnumNode, EventInfo, FloatNode, Frame, FrameGuard, FrameInfo, IntNode, MvsError,
    MvsResult, OwnedFrame, PixelType, Sdk, TransportLayer,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};

// -------------------------------------------------------------------------
// Thread-safety contract
// -------------------------------------------------------------------------

// SDK state and copied enumeration metadata may be shared between threads.
assert_impl_all!(Sdk: Send, Sync);
assert_impl_all!(DeviceList: std::fmt::Debug, Send, Sync);
assert_impl_all!(DeviceInfo<'static>: Copy, Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(DeviceIter<'static>: Iterator, ExactSizeIterator, Send, Sync);

// A camera handle may move to another thread, but one instance requires
// external synchronization for concurrent access.
assert_impl_all!(Camera: std::fmt::Debug, Send);
assert_not_impl_any!(Camera: Sync);

// A guard owns an SDK buffer tied to a mutable camera borrow and must remain
// on the acquiring thread.
assert_not_impl_any!(FrameGuard<'static>: Send, Sync);

// Borrowed frame views are safe to share for their valid lifetime, while an
// OwnedFrame is independent from all SDK-managed storage.
assert_impl_all!(Frame<'static>: std::fmt::Debug, Send, Sync);
assert_impl_all!(FrameInfo<'static>: Copy, Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(EventInfo<'static>: Copy, Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(OwnedFrame: Clone, std::fmt::Debug, Send, Sync);

// Public value and error types contain no platform-specific thread-affine
// state.
assert_impl_all!(AccessMode: Copy, Clone, std::fmt::Debug, PartialEq, Eq, Send, Sync);
assert_impl_all!(IntNode: Copy, Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(FloatNode: Copy, Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(EnumNode: Clone, std::fmt::Debug, Send, Sync);
assert_impl_all!(TransportLayer: Copy, Clone, std::fmt::Debug, Default, PartialEq, Eq, Send, Sync);
assert_impl_all!(PixelType: Copy, Clone, std::fmt::Debug, PartialEq, Eq, std::hash::Hash, Send, Sync);
assert_impl_all!(MvsError: std::error::Error, Send, Sync);
assert_impl_all!(CleanupStep: Copy, Clone, std::fmt::Debug, std::fmt::Display, PartialEq, Eq, Send, Sync);
assert_impl_all!(CleanupFailure: std::fmt::Debug, std::fmt::Display, std::error::Error, Send, Sync);
assert_impl_all!(CleanupError: std::fmt::Debug, std::fmt::Display, std::error::Error, Send, Sync);

// -------------------------------------------------------------------------
// Export paths and aliases
// -------------------------------------------------------------------------

fn assert_sized<T: Sized>() {}

fn all_public_types_are_nameable_from_the_crate_root() {
    // Each generic instantiation fails to compile if the corresponding type
    // disappears from the crate root or becomes private.
    assert_sized::<AccessMode>();
    assert_sized::<Camera>();
    assert_sized::<CleanupError>();
    assert_sized::<CleanupFailure>();
    assert_sized::<CleanupStep>();
    assert_sized::<DeviceInfo<'static>>();
    assert_sized::<DeviceIter<'static>>();
    assert_sized::<DeviceList>();
    assert_sized::<EnumNode>();
    assert_sized::<EventInfo<'static>>();
    assert_sized::<FloatNode>();
    assert_sized::<Frame<'static>>();
    assert_sized::<FrameGuard<'static>>();
    assert_sized::<FrameInfo<'static>>();
    assert_sized::<IntNode>();
    assert_sized::<MvsError>();
    assert_sized::<OwnedFrame>();
    assert_sized::<PixelType>();
    assert_sized::<Sdk>();
    assert_sized::<TransportLayer>();
}

fn error_export_contract(
    root_cleanup_error: CleanupError,
    root_cleanup_failure: CleanupFailure,
    root_cleanup_step: CleanupStep,
    root_error: MvsError,
    root_result: MvsResult<()>,
) {
    // The root exports and `mvs_sdk_rs::error` exports must remain aliases of
    // exactly the same types.
    let _: ModuleCleanupError = root_cleanup_error;
    let _: ModuleCleanupFailure = root_cleanup_failure;
    let _: ModuleCleanupStep = root_cleanup_step;
    let _: ModuleError = root_error;
    let _: ModuleResult<()> = root_result;
}

fn cleanup_error_api_contract(error: &CleanupError) {
    let _: &[CleanupFailure] = error.failures();
}

fn cleanup_error_consuming_api_contract(error: CleanupError) {
    let _: Vec<CleanupFailure> = error.into_failures();
}

fn cleanup_failure_api_contract(failure: &CleanupFailure) {
    let _: CleanupStep = failure.step;
    let _: &MvsError = &failure.error;
}

fn cleanup_step_api_contract() {
    let _: [CleanupStep; 7] = [
        CleanupStep::DrainCallbacks,
        CleanupStep::StopGrabbing,
        CleanupStep::UnregisterImageCallback,
        CleanupStep::UnregisterExceptionCallback,
        CleanupStep::UnregisterEventCallback,
        CleanupStep::CloseDevice,
        CleanupStep::DestroyHandle,
    ];
}

// -------------------------------------------------------------------------
// SDK and device API
// -------------------------------------------------------------------------

fn sdk_api_contract(sdk: &Arc<Sdk>) {
    let _: fn() -> MvsResult<Arc<Sdk>> = Sdk::init;
    let _: u32 = sdk.sdk_version();
    let _: MvsResult<DeviceList> =
        sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB);
}

fn device_list_api_contract(list: &DeviceList) {
    let _: usize = list.len();
    let _: bool = list.is_empty();
    let _: DeviceIter<'_> = list.iter();
    let _: Option<DeviceInfo<'_>> = list.get(0);
}

fn device_iterator_api_contract<'a>(mut iter: DeviceIter<'a>) {
    let _: Option<DeviceInfo<'a>> = iter.next();

    fn require_exact_size<I: ExactSizeIterator>(_iter: &I) {}
    require_exact_size(&iter);
}

fn device_info_api_contract(info: &DeviceInfo<'_>) {
    let _: TransportLayer = info.transport_layer();
    let _: bool = info.is_gige();
    let _: bool = info.is_usb();
    let _: String = info.manufacturer();
    let _: String = info.model();
    let _: String = info.serial();
    let _: String = info.user_defined_name();
    let _: Option<Ipv4Addr> = info.ip();
    let _: Option<Ipv4Addr> = info.host_nic_ip();
    let _: bool = info.is_accessible(AccessMode::Exclusive);
    let _: MvsResult<Camera> = info.open(AccessMode::Control);
    let _: MvsResult<Camera> = info.open_exclusive();
    let _: MvsResult<Camera> = info.open_control();

    // The safe crate exposes an opaque pointer instead of leaking a private
    // mvs-sdk-sys device-info type into its public signature.
    let _: *const c_void = info.as_raw();
}

// -------------------------------------------------------------------------
// Camera API
// -------------------------------------------------------------------------

fn camera_close_api_contract() {
    let _: fn(Camera) -> Result<(), CleanupError> = Camera::close;
}

fn camera_api_contract(camera: &mut Camera) {
    let _: *mut c_void = camera.as_raw_handle();
    let _: bool = camera.is_connected();
    let _: MvsResult<()> = camera.start_grabbing();

    // End the guard result's mutable camera borrow before probing the next
    // method. The function is never executed; only its types are checked.
    {
        let result: MvsResult<FrameGuard<'_>> = camera.get_image_buffer(100);
        drop(result);
    }
    let _: MvsResult<()> = camera.stop_grabbing();

    // Each closure deliberately combines two kinds of captured state:
    // - directly mutating a counter makes it FnMut-only rather than Fn;
    // - moving a Cell into it keeps the closure Send but makes it !Sync.
    // This catches accidental tightening of any callback to Fn or Sync.
    let mut exception_calls = 0_u32;
    let exception_state = Cell::new(0_u32);
    let _: MvsResult<()> = camera.register_exception_callback(move |message_type: u32| {
        exception_calls += 1;
        exception_state.set(message_type);
        let _: u32 = exception_calls;
    });
    let _: MvsResult<()> = camera.unregister_exception_callback();

    let mut event_calls = 0_u32;
    let event_state = Cell::new(0_u16);
    let _: MvsResult<()> =
        camera.register_event_callback("ExposureEnd", move |event: &EventInfo<'_>| {
            event_calls += 1;
            event_state.set(event.event_id());
            let _: u32 = event_calls;
        });
    let _: MvsResult<()> = camera.unregister_event_callback("ExposureEnd");

    let mut image_calls = 0_u32;
    let image_state = Cell::new(0_usize);
    let _: MvsResult<()> = camera.register_image_callback(move |frame: &Frame<'_>| {
        image_calls += 1;
        image_state.set(frame.data().len());
        let _: u32 = image_calls;
    });
    let _: MvsResult<()> = camera.unregister_image_callback();

    let _: MvsResult<()> = camera.event_notification_on("ExposureEnd");
    let _: MvsResult<()> = camera.event_notification_off("ExposureEnd");

    let _: MvsResult<()> = camera.set_int("Width", 1920);
    let _: MvsResult<i64> = camera.get_int("Width");
    // These three return types are intentionally explicit: this prevents a
    // regression of the original unnameable_types bug.
    let _: MvsResult<IntNode> = camera.get_int_range("Width");

    let _: MvsResult<()> = camera.set_float("ExposureTime", 5000.0);
    let _: MvsResult<f32> = camera.get_float("ExposureTime");
    let _: MvsResult<FloatNode> = camera.get_float_range("ExposureTime");

    let _: MvsResult<()> = camera.set_bool("ReverseX", false);
    let _: MvsResult<bool> = camera.get_bool("ReverseX");

    let _: MvsResult<()> = camera.set_enum("TriggerMode", "Off");
    let _: MvsResult<u32> = camera.get_enum("PixelFormat");
    let _: MvsResult<EnumNode> = camera.get_enum_info("PixelFormat");
    let _: MvsResult<()> = camera.set_enum_value("PixelFormat", 0);

    let _: MvsResult<()> = camera.set_string("DeviceUserID", "camera-1");
    let _: MvsResult<String> = camera.get_string("DeviceUserID");
    let _: MvsResult<()> = camera.exec_command("TriggerSoftware");
}

// -------------------------------------------------------------------------
// Frame and event API
// -------------------------------------------------------------------------

fn frame_api_contract(frame: &Frame<'_>) {
    let _: &[u8] = frame.data();
    let _: &FrameInfo<'_> = frame.info();
    let _: OwnedFrame = frame.to_owned();
}

fn frame_info_api_contract(info: &FrameInfo<'_>) {
    let _: u32 = info.width();
    let _: u32 = info.height();
    let _: PixelType = info.pixel_type();
    let _: u32 = info.frame_num();
    let _: u32 = info.frame_len();
    let _: u32 = info.offset_x();
    let _: u32 = info.offset_y();
    let _: f32 = info.gain();
    let _: f32 = info.exposure_time();
    let _: u32 = info.trigger_index();
    let _: u32 = info.lost_packets();
    let _: u64 = info.device_timestamp();
    let _: i64 = info.host_timestamp_raw();
    let _: Duration = info.host_timestamp();
}

fn owned_frame_api_contract(frame: &OwnedFrame) {
    let _: &[u8] = &frame.data;
    let _: FrameInfo<'_> = frame.info();
    let _: Frame<'_> = frame.as_frame();
}

fn frame_guard_api_contract(guard: FrameGuard<'_>) -> MvsResult<()> {
    {
        let frame: Frame<'_> = guard.frame();
        let _: &[u8] = frame.data();
    }
    {
        let info: FrameInfo<'_> = guard.info();
        let _: u32 = info.width();
    }
    let _: OwnedFrame = guard.to_owned();
    guard.release()
}

fn event_api_contract(event: &EventInfo<'_>) {
    let _: Cow<'_, str> = event.name();
    let _: u16 = event.event_id();
    let _: u16 = event.stream_channel();
    let _: u64 = event.block_id();
    let _: u64 = event.timestamp();
}

// -------------------------------------------------------------------------
// Public value types
// -------------------------------------------------------------------------

fn node_api_contract(int_node: &IntNode, float_node: &FloatNode, enum_node: &EnumNode) {
    let _: i64 = int_node.current;
    let _: i64 = int_node.min;
    let _: i64 = int_node.max;
    let _: i64 = int_node.inc;

    let _: f32 = float_node.current;
    let _: f32 = float_node.min;
    let _: f32 = float_node.max;

    let _: u32 = enum_node.current;
    let _: &[u32] = &enum_node.supported;
}

fn access_mode_api_contract() {
    let _: [AccessMode; 7] = [
        AccessMode::Exclusive,
        AccessMode::ExclusiveWithSwitch,
        AccessMode::Control,
        AccessMode::ControlWithSwitch,
        AccessMode::ControlSwitchEnable,
        AccessMode::ControlSwitchEnableWithKey,
        AccessMode::Monitor,
    ];
}

fn transport_layer_api_contract(mut layers: TransportLayer) {
    let _: [TransportLayer; 12] = [
        TransportLayer::UNKNOWN,
        TransportLayer::GIGE,
        TransportLayer::USB,
        TransportLayer::CAMERALINK,
        TransportLayer::VIR_GIGE,
        TransportLayer::VIR_USB,
        TransportLayer::GENTL_GIGE,
        TransportLayer::GENTL_CAMERALINK,
        TransportLayer::GENTL_CXP,
        TransportLayer::GENTL_XOF,
        TransportLayer::GENTL_VIR,
        TransportLayer::ALL,
    ];

    layers |= TransportLayer::USB;
    let _: TransportLayer = TransportLayer::from_raw(layers.raw());
    let _: bool = layers.contains(TransportLayer::USB);
}

fn pixel_type_api_contract(pixel: PixelType) {
    let _: [PixelType; 18] = [
        PixelType::UNDEFINED,
        PixelType::MONO8,
        PixelType::MONO10,
        PixelType::MONO10_PACKED,
        PixelType::MONO12,
        PixelType::MONO12_PACKED,
        PixelType::MONO14,
        PixelType::MONO16,
        PixelType::BAYER_GR8,
        PixelType::BAYER_RG8,
        PixelType::BAYER_GB8,
        PixelType::BAYER_BG8,
        PixelType::RGB8_PACKED,
        PixelType::BGR8_PACKED,
        PixelType::RGBA8_PACKED,
        PixelType::BGRA8_PACKED,
        PixelType::YUV422_PACKED,
        PixelType::YUV422_YUYV_PACKED,
    ];

    let _: PixelType = PixelType::from_raw(pixel.raw());
    let _: u32 = pixel.bits_per_pixel();
    let _: bool = pixel.is_mono();
    let _: bool = pixel.is_color();
    let _: bool = pixel.is_custom();
}
