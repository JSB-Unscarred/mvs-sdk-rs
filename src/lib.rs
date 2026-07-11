//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! Raw `unsafe` FFI is isolated in the companion `mvs-sdk-sys` crate. This
//! crate exposes one platform-independent safe API backed by the native SDK on
//! Windows x86_64 and an unsupported-platform backend elsewhere.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

pub(crate) use mvs_sdk_sys as sys;

mod backend;
mod callback;
mod camera;
mod device;
pub mod error;
mod frame;
mod library;
mod types;

pub use callback::EventInfo;
pub use camera::Camera;
pub use device::{DeviceInfo, DeviceIter, DeviceList};
pub use error::{MvsError, MvsResult};
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame};
pub use library::Sdk;
pub use types::{AccessMode, EnumNode, FloatNode, IntNode, PixelType, TransportLayer};

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_threading_contracts_hold_for_the_selected_backend() {
        assert_send::<Camera>();
        assert_send_sync::<Sdk>();
        assert_send_sync::<DeviceList>();
        assert_send_sync::<DeviceInfo<'static>>();
        assert_send_sync::<DeviceIter<'static>>();
        assert_send_sync::<EventInfo<'static>>();
        assert_send_sync::<Frame<'static>>();
        assert_send_sync::<FrameInfo<'static>>();
        assert_send_sync::<OwnedFrame>();
        assert_send_sync::<MvsError>();
    }
}
