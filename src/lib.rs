//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! Raw `unsafe` FFI is isolated in the companion `mvs-sdk-sys` crate. This
//! crate exposes one platform-independent API backed by the native SDK on
//! Windows x86_64. On other targets, [`Sdk::init`] returns
//! [`MvsError::UnsupportedPlatform`]. Windows applications need the MVS SDK,
//! `MVCAM_COMMON_RUNENV`, and the SDK DLL directory on `PATH` at runtime.
//!
//! # Workflow
//!
//! Initialize [`Sdk`], enumerate the desired [`TransportLayer`] values, open a
//! [`DeviceInfo`], configure GenICam nodes through [`Camera`], then choose one
//! acquisition mode:
//!
//! - Register an image callback before [`Camera::start_grabbing`] for callback
//!   mode. The SDK invokes it on a streaming thread and each [`Frame`] is
//!   borrowed only for that invocation.
//! - Start without an image callback for polling mode, then call
//!   [`Camera::get_image_buffer`]. Its [`FrameGuard`] releases the native buffer
//!   on drop; call [`FrameGuard::release`] to observe release errors.
//!
//! Stop acquisition before first registering or unregistering the image
//! callback, or before switching acquisition modes. An already registered
//! callback may be replaced while callback acquisition runs. To keep pixels
//! beyond a callback or guard lifetime, copy them with [`Frame::to_owned`].
//!
//! # Lifetimes and shutdown
//!
//! [`Camera`] is `Send` but not `Sync`; synchronize shared access externally.
//! Prefer [`Camera::close`] over relying on `Drop`, because explicit close can
//! report cleanup failure; use [`Camera::close_detailed`] to inspect every
//! failure. After all cameras are closed and callbacks have returned,
//! [`Sdk::shutdown`] can explicitly finalize the process-wide runtime.
//! Successful shutdown is terminal for the process.
//!
//! See the repository's `examples/callback.rs` and `examples/polling.rs` for
//! complete acquisition workflows.

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
pub use error::{CleanupError, MvsError, MvsResult, ShutdownError};
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame};
pub use library::Sdk;
pub use types::{AccessMode, EnumNode, FloatNode, IntNode, PixelType, TransportLayer};
