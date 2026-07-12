//! Runtime behavior of the private unsupported-platform backend.
//!
//! Windows x86_64 uses the native SDK backend, so this test file is empty for
//! that target and never attempts to initialize real MVS hardware.

#![cfg(not(all(target_os = "windows", target_arch = "x86_64")))]

use mvs_sdk_rs::{MvsError, Sdk};

#[test]
fn sdk_init_returns_unsupported_platform() {
    // Unsupported targets must fail explicitly instead of constructing a fake
    // SDK or returning fabricated device information.
    assert!(matches!(Sdk::init(), Err(MvsError::UnsupportedPlatform)));
}

#[test]
fn unsupported_platform_has_no_native_error_code() {
    // This error originates in the safe wrapper and therefore has no MV_E_*
    // code from the vendor SDK.
    assert_eq!(MvsError::UnsupportedPlatform.raw_code(), None);
}
