//! 列出 `GigE` 与 USB3 相机。

use mvs_sdk::{Sdk, TransportLayer};

fn main() -> mvs_sdk::Result<()> {
    let version = Sdk::version();
    println!("MVS SDK 0x{version:08X}");

    let sdk = Sdk::new()?;
    for device in sdk.devices(TransportLayer::GIGE | TransportLayer::USB)? {
        println!(
            "{:<24} SN {:<16} {:?}",
            device.model_name().to_string_lossy(),
            device.serial_number().to_string_lossy(),
            device.current_ip(),
        );
    }
    Ok(())
}
