//! Windows x64 + MVS SDK 真机显式 buffer 归还测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;

// 验证显式 release 会归还一个 SDK buffer，允许在另一 buffer 被占用时继续取图。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn explicit_release_returns_one_of_two_stream_buffers() -> Result<(), Box<dyn Error>> {
    let (_sdk, mut camera) = hardware_support::open_test_camera()?;
    hardware_support::require_trigger_off(&camera)?;
    hardware_support::set_two_stream_buffers(&camera)?;

    camera.start_grabbing()?;
    let first = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    let second = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    first.release()?;

    let third = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    assert_eq!(
        third.frame().data().len(),
        third.info().frame_len() as usize
    );
    second.release()?;
    third.release()?;
    camera.stop_grabbing()?;
    camera.close()?;
    Ok(())
}
