//! 打开第一台相机，以 polling 方式取 10 帧。

use std::time::Duration;

use mvs_sdk_rs::{AccessMode, Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::initialize()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices.first().ok_or("没有找到相机")?;

    let mut camera = sdk.open(device, AccessMode::Exclusive, 0)?;
    camera.set_enum_symbolic(c"TriggerMode", c"Off")?;

    let grabbing = camera.start_grabbing()?;
    for _ in 0..10 {
        let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
        let frame = buffer.frame();
        let info = frame.info();
        println!(
            "#{} {}x{} {} 字节",
            info.frame_number,
            info.width,
            info.height,
            frame.data().len()
        );
    }
    grabbing.stop()?;

    camera.close()?;
    Ok(())
}
