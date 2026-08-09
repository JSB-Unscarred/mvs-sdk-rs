//! 安全敏感 public API 的 compile-time contract。
//!
//! Cargo 将 integration test 作为 downstream crate 编译。这些函数无需执行；
//! 编译本身验证 ownership 与 callback bounds，同时避免调用 native MVS SDK。

#![allow(dead_code)]

use std::cell::Cell;

use mvs_sdk_rs::{Camera, EventInfo, Frame, FrameGuard, MvsResult, OwnedFrame, Sdk};
use static_assertions::{assert_impl_all, assert_not_impl_any};

// 验证 SDK state 可共享，而 Camera 的可变状态需要调用方同步。
assert_impl_all!(Sdk: Send, Sync);
assert_impl_all!(Camera: Send);
assert_not_impl_any!(Camera: Sync);

// 验证 SDK buffer 仅在取图线程使用，OwnedFrame 与 SDK storage 解耦后可跨线程。
assert_not_impl_any!(FrameGuard<'static>: Send, Sync);
assert_impl_all!(OwnedFrame: Send, Sync);

// 验证 callback 接受 FnMut + Send closure，且不额外要求 Sync。
fn callbacks_accept_fn_mut_send_but_not_sync(camera: &mut Camera) {
    // 修改 counter 使 closure 仅实现 FnMut；捕获 Cell 使其 Send + !Sync。
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
