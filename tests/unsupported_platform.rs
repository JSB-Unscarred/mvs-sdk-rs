//! 非 Windows x86_64 MSVC 平台的初始化行为。

#![cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]

use mvs_sdk_rs::{MvsError, Sdk};

// 验证 unsupported backend 重复调用也只返回平台错误。
#[test]
fn sdk_init_repeatedly_reports_unsupported_platform() {
    for _ in 0..2 {
        let Err(error) = Sdk::init() else {
            panic!("unsupported targets must not construct an SDK");
        };

        assert!(matches!(&error, MvsError::UnsupportedPlatform));
    }
}
