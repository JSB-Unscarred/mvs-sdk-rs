//! 真机数据流测试：需要 MVS SDK 与一台专用测试相机，运行方式：
//!
//! ```text
//! $env:MVS_SDK_TEST_SERIAL = "<相机序列号>"
//! cargo test --test hardware_smoke -- --ignored
//! ```
//!
//! `cargo test --workspace` 会启动本测试程序，没有 `MvCameraControl.dll` 时程序无法加载；
//! 未安装 SDK 的机器改用 `cargo test --workspace --lib --test thread_traits`。

use std::error::Error;
use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

use mvs_sdk::{AccessMode, CallbackGrabbing, Sdk, TransportLayer};

const TIMEOUT: Duration = Duration::from_secs(3);

// pull（借用相机）与 callback（按值持有相机）两条取流链，以及显式清理。
#[test]
#[ignore = "requires a dedicated camera and MVS_SDK_TEST_SERIAL"]
fn pull_and_callback_grabbing() -> Result<(), Box<dyn Error>> {
    // 只操作专用测试相机，避免误用其它设备。
    let serial = CString::new(std::env::var("MVS_SDK_TEST_SERIAL")?)?;
    let sdk = Sdk::new()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices
        .iter()
        .find(|device| device.serial_number() == serial.as_c_str())
        .ok_or("the test camera was not enumerated")?;

    let mut camera = sdk.open(device, AccessMode::Exclusive, 0)?;
    // free-run 才能在超时内拿到图像。
    camera.set_enum_symbolic(c"TriggerMode", c"Off")?;

    let grabbing = camera.start_grabbing()?;
    {
        let buffer = grabbing.get_image_buffer(Some(TIMEOUT))?;
        assert!(!buffer.frame().data.is_empty());
    }
    grabbing.stop().1?;

    // callback 守卫按值持有相机，stop 后交还。
    let (sender, receiver) = mpsc::sync_channel(1);
    let grabbing = CallbackGrabbing::start(camera, move |frame| {
        let _ = sender.try_send(frame.data.to_vec());
    })
    .map_err(|(_, error)| error)?;
    assert!(!receiver.recv_timeout(TIMEOUT)?.is_empty());
    let (camera, result) = grabbing.stop();
    result?;

    camera.close()?;
    Ok(())
}
