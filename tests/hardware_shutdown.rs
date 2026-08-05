//! Windows x64 + MVS SDK 真机 finalization 测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

use std::error::Error;

use mvs_sdk_rs::Sdk;

// 验证真实 SDK 可在独立进程中完成一次终态 shutdown。
#[test]
#[ignore = "requires the MVS SDK"]
fn real_sdk_can_shutdown() -> Result<(), Box<dyn Error>> {
    let sdk = Sdk::init()?;
    sdk.shutdown()?;
    Ok(())
}
