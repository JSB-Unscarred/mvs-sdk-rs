//! 打开第一台相机，在 callback 中复制像素并交给主线程。

use std::sync::mpsc;
use std::time::Duration;

use mvs_sdk::{AccessMode, ExceptionKind, Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::new()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices.first().ok_or("no camera found")?;

    let mut camera = sdk.open(device, AccessMode::Exclusive, 0)?;
    camera.register_exception_callback(|kind| {
        if kind == ExceptionKind::Disconnected {
            eprintln!("camera disconnected");
        }
    })?;

    // callback 在 SDK 线程运行：只复制数据，处理交给主线程。
    let (sender, receiver) = mpsc::sync_channel(4);
    let grabbing = camera.start_grabbing_with(move |frame| {
        let _ = sender.try_send((frame.info, frame.data.to_vec()));
    })?;
    for _ in 0..10 {
        let (info, pixels) = receiver.recv_timeout(Duration::from_secs(1))?;
        println!(
            "#{} {}x{} {} bytes",
            info.frame_number,
            info.width,
            info.height,
            pixels.len()
        );
    }
    grabbing.stop().1?;

    camera.close()?;
    Ok(())
}
