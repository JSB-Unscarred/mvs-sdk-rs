//! 真机数据流测试：需要 MVS SDK、专用相机与 `MVS_TEST_CAMERA_SERIAL`。

use std::error::Error;
use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

use mvs_sdk_rs::{AccessMode, Sdk, TransportLayer};

const TIMEOUT: Duration = Duration::from_secs(3);

// polling 与 callback 两条取流链，以及显式清理。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn real_camera_data_flow() -> Result<(), Box<dyn Error>> {
    // 只操作专用测试相机，避免误用其它设备。
    let serial = CString::new(std::env::var("MVS_TEST_CAMERA_SERIAL")?)?;
    let sdk = Sdk::initialize()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices
        .iter()
        .find(|device| device.serial_number() == serial.as_c_str())
        .ok_or("the test camera was not enumerated")?;

    let mut camera = sdk.open(device, AccessMode::Exclusive, 0)?;
    // free-run 才能在超时内拿到图像。
    assert_eq!(camera.get_enum(c"TriggerMode")?.current, 0, "TriggerMode must be Off");

    let grabbing = camera.start_grabbing()?;
    {
        let buffer = grabbing.get_image_buffer(Some(TIMEOUT))?;
        assert!(!buffer.frame().data().is_empty());
    }
    grabbing.stop()?;

    let (sender, receiver) = mpsc::sync_channel(1);
    let grabbing = camera.start_grabbing_with(move |frame| {
        let _ = sender.try_send(frame.data().to_vec());
    })?;
    assert!(!receiver.recv_timeout(TIMEOUT)?.is_empty());
    grabbing.stop()?;

    camera.close()?;
    Ok(())
}
