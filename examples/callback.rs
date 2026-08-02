use std::{
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

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

    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    camera.register_image_callback(move |frame| {
        let info = frame.info();
        let _ = frame_tx.try_send((
            info.frame_num(),
            info.width(),
            info.height(),
            frame.data().len(),
        ));
    })?;

    camera.start_grabbing()?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((frame_num, width, height, data_len)) => {
                println!("frame={frame_num} size={width}x{height} bytes={data_len}");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    camera.stop_grabbing()?;
    camera.close()?;
    sdk.shutdown()?;

    Ok(())
}
