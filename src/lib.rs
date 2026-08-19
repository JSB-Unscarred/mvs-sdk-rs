//! Safe Rust wrapper for the Hikvision **MVS** machine-vision camera SDK.
//!
//! Raw `unsafe` FFI is isolated in the companion `mvs-sdk-sys` crate. This
//! crate exposes one platform-independent API backed by the native SDK on
//! Windows x86_64 MSVC. On other targets, [`Sdk::initialize`] returns
//! [`MvsError::UnsupportedPlatform`]. Building and linking Windows MSVC applications
//! requires the MVS SDK and `MVCAM_COMMON_RUNENV`; at runtime, the SDK DLL
//! directory must be discoverable by the Windows loader, typically via `PATH`.
//!
//! # Workflow
//!
//! Initialize [`Sdk`], discover owned [`DeviceInfo`] snapshots with
//! [`Sdk::devices`], open one through [`Sdk::open`], configure GenICam nodes
//! through [`Camera`], then choose one acquisition mode:
//!
//! - Register an image callback before [`Camera::start_grabbing`] for callback
//!   mode. The SDK invokes it on a streaming thread and each [`Frame`] is
//!   borrowed only for that invocation.
//! - Start without an image callback for polling mode, then call
//!   [`Camera::get_image_buffer`]. Its [`FrameGuard`] releases the native buffer
//!   on drop; call [`FrameGuard::release`] to observe release errors, or use
//!   [`Camera::get_owned_frame`] to copy and explicitly release in one call.
//!
//! Stop acquisition before registering or unregistering the image
//! callback, and before switching acquisition modes. To keep pixels beyond a
//! callback or guard lifetime, copy them with [`Frame::to_owned`].
//! A callback must ask the [`Camera`] owner thread to change lifecycle state;
//! while the current thread is in any MVS callback, direct lifecycle changes
//! report a local [`MvsError::InvalidState`] without entering the native SDK
//! (`Camera::close` reports it through `CleanupError`).
//!
//! # Lifetimes and shutdown
//!
//! [`Camera`] owns an internal lease on the process-wide session and is `Send` but not `Sync`;
//! move its unique owner to a worker thread and serialize access to one handle.
//! Prefer [`Camera::close`] over relying on `Drop`, because explicit close can
//! preserve the first pre-destroy operation/error and the Destroy error in
//! `CleanupError`. `Camera::close`, [`FrameGuard::release`] and [`Sdk::shutdown`]
//! consume their owner and attempt cleanup once; their errors are diagnostic
//! input for host policy, not retry handles.
//! Consuming [`Sdk`] with [`Sdk::shutdown`] succeeds only after all cameras are closed or dropped.
//! A native handle whose destruction was not confirmed also blocks Finalize after its Rust owner
//! is consumed.
//! Finalization is terminal for the process, as required by the vendor
//! documentation.
//!
//! See the repository's `tests/hardware_smoke.rs` for polling and callback
//! workflows on separate native handles.

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
pub use device::DeviceInfo;
pub use error::{CleanupError, MvsError, MvsResult};
pub use frame::{Frame, FrameGuard, FrameInfo, OwnedFrame};
pub use library::Sdk;
pub use types::{AccessMode, EnumValue, FloatValue, IntValue, PixelType, TransportLayer};
