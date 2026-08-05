//! Windows x64 + MVS SDK 真机枚举测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;

use mvs_sdk_rs::{AccessMode, Sdk};

// 验证真实 SDK 能枚举指定设备，并报告 exclusive access 可用。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and a dedicated camera"]
fn real_device_is_enumerated_and_accessible() -> Result<(), Box<dyn Error>> {
    let sdk = Sdk::init()?;
    let device = hardware_support::test_device(&sdk)?;

    assert!(device.is_accessible(AccessMode::Exclusive)?);
    Ok(())
}
