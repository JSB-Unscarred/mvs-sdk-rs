//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! Raw `unsafe` FFI is isolated in the companion `mvs-sdk-sys` crate; this
//! crate exposes a safe Rust API.
//!
//! # Platform support
//!
//! Windows x86_64 only. On other targets the crate exposes stub APIs so that
//! `cargo check` works in cross-platform workspaces.
//!
//! See the crate README for a usage example.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) use mvs_sdk_sys as sys;

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod callback;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod camera;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod device;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub mod error;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod frame;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod library;

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use callback::EventInfo;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use camera::{AccessMode, Camera, EnumNode, FloatNode, IntNode};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use device::{DeviceInfo, DeviceIter, DeviceList, TransportLayer};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use error::{MvsError, MvsResult};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame, PixelType};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub use library::Sdk;

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
mod stub;
#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub use stub::*;
