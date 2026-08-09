//! Windows x64 + MVS SDK 真机完整数据流测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;
use std::sync::mpsc;
use std::time::Duration;

// 验证枚举、节点访问、两种 polling buffer 归还、callback 切换及终态 shutdown。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn real_camera_data_flow_smoke() -> Result<(), Box<dyn Error>> {
    let (sdk, mut camera) = hardware_support::open_test_camera()?;

    let width = camera.get_int_range("Width")?;
    assert!((width.min..=width.max).contains(&width.current));
    assert!(width.inc > 0);

    let exposure = camera.get_float_range("ExposureTime")?;
    assert!(exposure.current.is_finite());
    assert!(exposure.min <= exposure.current && exposure.current <= exposure.max);

    let pixel_format = camera.get_enum_info("PixelFormat")?;
    assert!(pixel_format.supported.contains(&pixel_format.current));

    hardware_support::require_trigger_off(&camera)?;
    hardware_support::set_two_stream_buffers(&camera)?;

    camera.start_grabbing()?;
    let first = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    let second = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    first.release()?;

    let third = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    drop(second);
    let fourth = camera.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
    assert_eq!(
        third.frame().data().len(),
        third.info().frame_len() as usize
    );
    assert_eq!(
        fourth.frame().data().len(),
        fourth.info().frame_len() as usize
    );
    third.release()?;
    fourth.release()?;
    camera.stop_grabbing()?;

    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    camera.register_image_callback(move |frame| {
        let _ = frame_tx.try_send(frame.to_owned());
    })?;
    camera.start_grabbing()?;
    let owned = frame_rx.recv_timeout(Duration::from_millis(u64::from(
        hardware_support::FRAME_TIMEOUT_MS,
    )))?;
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
    sdk.shutdown()?;
    Ok(())
}
