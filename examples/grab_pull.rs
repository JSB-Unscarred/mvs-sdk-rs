//! 打开第一台相机，以 pull 方式取 10 帧。

use std::time::Duration;

use mvs_sdk::{AccessMode, Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::new()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices.first().ok_or("no camera found")?;

    let mut camera = sdk.open(device, AccessMode::Exclusive, 0)?;
    camera.set_enum_symbolic(c"TriggerMode", c"Off")?;

    let grabbing = camera.start_grabbing()?;
    for _ in 0..10 {
        let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
        let frame = buffer.frame();
        let info = frame.info;
        println!(
            "#{} {}x{} {} bytes",
            info.frame_number,
            info.width,
            info.height,
            frame.data.len()
        );
    }
    grabbing.stop().1?;

    camera.close()?;
    Ok(())
}
