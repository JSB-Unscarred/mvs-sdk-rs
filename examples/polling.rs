use mvs_sdk_rs::{Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::init()?;
    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let Some(device) = devices.iter().next() else {
        println!("No camera found");
        sdk.shutdown()?;
        return Ok(());
    };

    let mut camera = device.open_exclusive()?;
    camera.set_enum("TriggerMode", "Off")?;
    camera.start_grabbing()?;

    let guard = camera.get_image_buffer(1000)?;
    let frame = guard.frame();
    let info = frame.info();
    println!(
        "frame={} size={}x{} bytes={}",
        info.frame_num(),
        info.width(),
        info.height(),
        frame.data().len()
    );
    let owned = frame.to_owned();
    guard.release()?;

    println!("copied {} bytes", owned.data.len());
    camera.stop_grabbing()?;
    camera.close()?;
    sdk.shutdown()?;

    Ok(())
}
