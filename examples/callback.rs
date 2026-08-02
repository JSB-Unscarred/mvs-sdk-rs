use std::time::Duration;

use mvs_sdk_rs::{Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::init()?;
    println!("MVS SDK version: 0x{:08X}", sdk.sdk_version());

    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let Some(device) = devices.iter().next() else {
        println!("No camera found");
        sdk.shutdown()?;
        return Ok(());
    };

    println!(
        "Open camera: {} {} SN={}",
        device.manufacturer(),
        device.model(),
        device.serial()
    );

    let mut camera = device.open_exclusive()?;
    camera.set_enum("TriggerMode", "Off")?;
    camera.set_float("ExposureTime", 5000.0)?;
    camera.register_image_callback(|frame| {
        let info = frame.info();
        println!(
            "frame={} size={}x{} bytes={}",
            info.frame_num(),
            info.width(),
            info.height(),
            frame.data().len()
        );
    })?;

    camera.start_grabbing()?;
    std::thread::sleep(Duration::from_secs(3));
    camera.stop_grabbing()?;
    camera.close()?;
    sdk.shutdown()?;

    Ok(())
}
