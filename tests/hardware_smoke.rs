//! Windows x64 + MVS SDK 真机完整数据流测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;
use std::sync::mpsc;
use std::time::Duration;

use mvs_sdk_rs::{AccessMode, Sdk, TransportLayer};

// 验证 polling、callback 两条核心取流链和完整资源清理。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn real_camera_data_flow_smoke() -> Result<(), Box<dyn Error>> {
    let sdk = Sdk::init()?;
    {
        let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
        let device = hardware_support::test_device(&devices)?;

        let mut polling = device.open(AccessMode::Exclusive, 0)?;
        hardware_support::require_trigger_off(&polling)?;
        polling.start_grabbing()?;
        let frame = polling.get_image_buffer(hardware_support::FRAME_TIMEOUT_MS)?;
        assert_eq!(
            frame.frame().data().len(),
            frame.info().frame_len() as usize
        );
        frame.release()?;
        polling.stop_grabbing()?;
        polling.close()?;

        let mut callback = device.open(AccessMode::Exclusive, 0)?;
        hardware_support::require_trigger_off(&callback)?;
        let (frame_tx, frame_rx) = mpsc::sync_channel(1);
        callback.register_image_callback(move |frame| {
            let _ = frame_tx.try_send(frame.to_owned());
        })?;
        callback.start_grabbing()?;
        let owned = frame_rx.recv_timeout(Duration::from_millis(u64::from(
            hardware_support::FRAME_TIMEOUT_MS,
        )))?;
        assert_eq!(owned.data().len(), owned.info().frame_len() as usize);
        callback.stop_grabbing()?;
        callback.close()?;
    }
    sdk.shutdown()?;
    Ok(())
}
