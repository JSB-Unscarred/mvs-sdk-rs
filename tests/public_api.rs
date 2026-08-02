//! Focused compile-time contracts for the safety-sensitive public API.
//!
//! Cargo compiles integration tests as downstream crates. These functions are
//! intentionally never run: compiling them verifies ownership and callback
//! bounds without linking calls to the native MVS SDK.

#![allow(dead_code)]

use std::cell::Cell;

use mvs_sdk_rs::{
    AccessMode, Camera, CleanupError, EventInfo, Frame, FrameGuard, MvsResult, OwnedFrame, Sdk,
    ShutdownError,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};

// SDK state may be shared, while a camera needs external synchronization.
assert_impl_all!(Sdk: Send, Sync);
assert_impl_all!(Camera: Send);
assert_not_impl_any!(Camera: Sync);

// SDK buffers must be released on the acquiring thread. A copied frame is
// detached from SDK-managed storage and can be transferred freely.
assert_not_impl_any!(FrameGuard<'static>: Send, Sync);
assert_impl_all!(OwnedFrame: Send, Sync);

// Explicit close reports a simple error by default and preserves full cleanup
// diagnostics through the opt-in API.
fn camera_close_contract() {
    let _: fn(Camera) -> MvsResult<()> = Camera::close;
    let _: fn(Camera) -> Result<(), CleanupError> = Camera::close_detailed;
}

fn sdk_shutdown_contract(sdk: &Sdk) {
    let _: Result<(), ShutdownError> = sdk.shutdown();
}

// A guard supports a detached copy before its consuming, fallible release.
fn frame_guard_ownership_contract(guard: FrameGuard<'_>) -> MvsResult<()> {
    let _: Frame<'_> = guard.frame();
    let _: u64 = guard.info().frame_len();
    let _: OwnedFrame = guard.to_owned();
    guard.release()
}

fn owned_frame_data_contract(mut frame: OwnedFrame) {
    let _: &[u8] = frame.data();
    let _: &mut [u8] = frame.data_mut();
    let _: Vec<u8> = frame.into_data();
}

fn keyed_access_mode_contract() {
    let _: AccessMode = AccessMode::ControlSwitchEnableWithKey(0x1234);
}

fn callbacks_accept_fn_mut_send_but_not_sync(camera: &mut Camera) {
    // Mutating the counter makes each closure FnMut-only. Capturing Cell keeps
    // it Send but makes it !Sync, guarding against either bound being tightened.
    let mut exception_calls = 0_u32;
    let exception_state = Cell::new(0_u32);
    let _: MvsResult<()> = camera.register_exception_callback(move |message_type| {
        exception_calls = exception_calls.wrapping_add(1);
        exception_state.set(message_type.wrapping_add(exception_calls));
    });

    let mut event_calls = 0_u16;
    let event_state = Cell::new(0_u16);
    let _: MvsResult<()> =
        camera.register_event_callback("ExposureEnd", move |event: &EventInfo<'_>| {
            event_calls = event_calls.wrapping_add(1);
            event_state.set(event.event_id().wrapping_add(event_calls));
        });

    let mut image_calls = 0_usize;
    let image_state = Cell::new(0_usize);
    let _: MvsResult<()> = camera.register_image_callback(move |frame: &Frame<'_>| {
        image_calls = image_calls.wrapping_add(1);
        image_state.set(frame.data().len().wrapping_add(image_calls));
    });
}
