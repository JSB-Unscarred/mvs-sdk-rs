//! 非 Windows x86_64 平台的初始化行为。

#![cfg(not(all(target_os = "windows", target_arch = "x86_64")))]

use mvs_sdk_rs::{MvsError, Sdk};

// 验证 unsupported backend 明确返回平台错误。
#[test]
fn sdk_init_reports_unsupported_platform() {
    let Err(error) = Sdk::init() else {
        panic!("unsupported targets must not construct an SDK");
    };

    assert!(matches!(&error, MvsError::UnsupportedPlatform));
}
