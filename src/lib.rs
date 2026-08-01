//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! Raw `unsafe` FFI is isolated in the companion `mvs-sdk-sys` crate. This
//! crate exposes one platform-independent safe API backed by the native SDK on
//! Windows x86_64 and an unsupported-platform backend elsewhere.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
// Public functions must not expose types that downstream crates cannot name.
#![deny(unnameable_types)]
// Platform backends are private; accidentally writing `pub` inside one should
// fail compilation instead of silently creating an unreachable public item.
#![deny(unreachable_pub)]

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
pub use error::{CleanupError, CleanupFailure, CleanupStep, MvsError, MvsResult};
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame};
pub use library::Sdk;
pub use types::{AccessMode, EnumNode, FloatNode, IntNode, PixelType, TransportLayer};
