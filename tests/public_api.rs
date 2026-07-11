#![cfg(not(all(target_os = "windows", target_arch = "x86_64")))]

// A Windows integration-test binary must link the proprietary MVS import
// library. The same positive auto-trait contract is also checked by the safe
// crate's unit tests, which do not require that external linker input.

use mvs_sdk_rs::error::{MvsError as ModuleError, MvsResult as ModuleResult};
use mvs_sdk_rs::{
    AccessMode, Camera, DeviceInfo, DeviceIter, DeviceList, EnumNode, EventInfo, FloatNode, Frame,
    FrameGuard, FrameInfo, IntNode, MvsError, MvsResult, OwnedFrame, PixelType, Sdk,
    TransportLayer,
};

fn assert_send<T: Send>() {}
fn assert_send_sync<T: Send + Sync>() {}
fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}

#[test]
fn public_types_are_nameable_from_the_same_paths() {
    let _ = std::mem::size_of::<AccessMode>();
    let _ = std::mem::size_of::<Camera>();
    let _ = std::mem::size_of::<DeviceInfo<'static>>();
    let _ = std::mem::size_of::<DeviceIter<'static>>();
    let _ = std::mem::size_of::<DeviceList>();
    let _ = std::mem::size_of::<EnumNode>();
    let _ = std::mem::size_of::<EventInfo<'static>>();
    let _ = std::mem::size_of::<FloatNode>();
    let _ = std::mem::size_of::<Frame<'static>>();
    let _ = std::mem::size_of::<FrameGuard<'static>>();
    let _ = std::mem::size_of::<FrameInfo<'static>>();
    let _ = std::mem::size_of::<IntNode>();
    let _ = std::mem::size_of::<OwnedFrame>();
    let _ = std::mem::size_of::<PixelType>();
    let _ = std::mem::size_of::<Sdk>();
    let _ = std::mem::size_of::<TransportLayer>();

    let _: Option<MvsResult<()>> = None;
    let _: Option<ModuleResult<()>> = None;
    let _: Option<MvsError> = None;
    let _: Option<ModuleError> = None;
}

#[test]
fn documented_threading_contracts_hold() {
    assert_send::<Camera>();
    assert_send_sync::<DeviceList>();
    assert_send_sync::<EventInfo<'static>>();
    assert_send_sync::<Frame<'static>>();
    assert_send_sync::<FrameInfo<'static>>();
    assert_send_sync::<OwnedFrame>();
    assert_error::<MvsError>();
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
#[test]
fn unsupported_platform_has_a_stable_error() {
    assert!(matches!(Sdk::init(), Err(MvsError::UnsupportedPlatform)));
}
