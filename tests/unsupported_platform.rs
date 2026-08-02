//! Runtime behavior of the private unsupported-platform backend.
//!
//! Windows x86_64 uses the native SDK backend, so this test file is empty for
//! that target and never attempts to initialize real MVS hardware.

#![cfg(not(all(target_os = "windows", target_arch = "x86_64")))]

use mvs_sdk_rs::{MvsError, Sdk};

#[test]
fn sdk_init_reports_unsupported_platform_without_a_native_code() {
    // Unsupported targets must fail explicitly instead of constructing a fake
    // SDK or returning fabricated device information.
    let Err(error) = Sdk::init() else {
        panic!("unsupported targets must not construct an SDK");
    };

    assert!(matches!(&error, MvsError::UnsupportedPlatform));
    assert_eq!(error.raw_code(), None);
}
