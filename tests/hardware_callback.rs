//! Windows x64 + MVS SDK 真机 image callback 生命周期测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;
use std::sync::mpsc;
use std::time::Duration;

// 验证 callback 可复制 frame，注销后同一 handle 可恢复 polling 取图。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn image_callback_can_be_unregistered_before_polling() -> Result<(), Box<dyn Error>> {
    let (_sdk, mut camera) = hardware_support::open_test_camera()?;
    hardware_support::require_trigger_off(&camera)?;

    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    camera.register_image_callback(move |frame| {
        let _ = frame_tx.try_send(frame.to_owned());
    })?;
    camera.start_grabbing()?;
    let owned = frame_rx.recv_timeout(Duration::from_secs(3))?;
    assert_eq!(owned.data().len(), owned.info().frame_len() as usize);
    camera.stop_grabbing()?;
    camera.unregister_image_callback()?;

    camera.start_grabbing()?;
    let polled = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    assert_eq!(
        polled.frame().data().len(),
        polled.info().frame_len() as usize
    );
    polled.release()?;
    camera.stop_grabbing()?;
    camera.close()?;
    Ok(())
}
