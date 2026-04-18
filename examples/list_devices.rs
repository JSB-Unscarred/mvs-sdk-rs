//! Enumerate connected MVS cameras and print their metadata.
//!
//! Run with:
//!   cargo run --example list_devices

use mvs_wrapper::{AccessMode, Library, MvsResult, TransportLayer};

fn main() -> MvsResult<()> {
    let lib = Library::init()?;
    let version = lib.sdk_version();
    println!("MVS SDK version: 0x{:08X}", version);

    let devices =
        lib.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;

    if devices.is_empty() {
        println!("No cameras found. Check cables, power, and driver.");
        return Ok(());
    }

    println!("Found {} device(s):\n", devices.len());
    for (i, dev) in devices.iter().enumerate() {
        println!("  [{i}] transport = {:?}", dev.transport_layer());
        println!("      manufacturer      : {}", dev.manufacturer());
        println!("      model             : {}", dev.model());
        println!("      serial            : {}", dev.serial());
        let user_name = dev.user_defined_name();
        if !user_name.is_empty() {
            println!("      user-defined name : {}", user_name);
        }
        if let Some(ip) = dev.ip() {
            println!("      current IP        : {}", ip);
        }
        if let Some(nic) = dev.host_nic_ip() {
            println!("      host NIC IP       : {}", nic);
        }
        let accessible = dev.is_accessible(AccessMode::Exclusive);
        println!("      exclusive-open OK : {}", accessible);
        println!();
    }

    Ok(())
}
