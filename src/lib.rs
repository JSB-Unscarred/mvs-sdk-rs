//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! All `unsafe` FFI is contained within this crate; consumer code is 100% safe Rust.
//!
//! # Platform support
//!
//! Windows x86_64 only. On other targets the crate exposes stub APIs so that
//! `cargo check` works in cross-platform workspaces.
//!
//! See the crate README for a usage example.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

#[cfg(mvs_platform)]
pub(crate) mod sys;

#[cfg(mvs_platform)]
mod callback;
#[cfg(mvs_platform)]
mod camera;
#[cfg(mvs_platform)]
mod device;
#[cfg(mvs_platform)]
pub mod error;
#[cfg(mvs_platform)]
mod frame;
#[cfg(mvs_platform)]
mod library;

#[cfg(mvs_platform)]
pub use callback::EventInfo;
#[cfg(mvs_platform)]
pub use camera::{AccessMode, Camera};
#[cfg(mvs_platform)]
pub use device::{DeviceInfo, DeviceIter, DeviceList, TransportLayer};
#[cfg(mvs_platform)]
pub use error::{MvsError, MvsResult};
#[cfg(mvs_platform)]
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame, PixelType};
#[cfg(mvs_platform)]
pub use library::Sdk;

#[cfg(not(mvs_platform))]
mod stub;
#[cfg(not(mvs_platform))]
pub use stub::*;
