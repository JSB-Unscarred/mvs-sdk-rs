//! 海康威视 MVS 工业相机 SDK 的安全 Rust 封装。
//!
//! 原始 FFI 位于 `mvs-sdk-sys`。本 crate 用所有权与借用表达 SDK 的调用约定：
//!
//! - [`Sdk`] 初始化进程级 SDK，枚举并打开设备；
//! - [`Camera`] 独占一个 native handle，负责节点读写和 exception/event callback；
//! - [`Camera::start_grabbing`] 与 [`Camera::start_grabbing_with`] 返回借用相机的取流守卫，
//!   polling 取图只存在于 [`Grabbing`] 上，守卫释放时停止取流。
//!
//! 设计取舍见 [`docs::architecture`]。
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use mvs_sdk::{AccessMode, Sdk, TransportLayer};
//!
//! fn main() -> mvs_sdk::Result<()> {
//!     let sdk = Sdk::initialize()?;
//!     let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
//!     let mut camera = sdk.open(&devices[0], AccessMode::Exclusive, 0)?;
//!     camera.set_float(c"ExposureTime", 10_000.0)?;
//!
//!     let grabbing = camera.start_grabbing()?;
//!     let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
//!     let frame = buffer.frame();
//!     println!("{:?}，{} 字节", frame.info(), frame.data().len());
//!     Ok(())
//! }
//! ```

use std::ffi::CStr;
use std::os::raw::c_char;

pub(crate) use mvs_sdk_sys as sys;

mod callback;
mod camera;
mod device;
pub mod docs;
mod error;
mod frame;
mod grabbing;
mod kind;
mod sdk;

pub use callback::{EventInfo, ExceptionKind};
pub use camera::{Camera, EnumValue, FloatValue, IntValue};
pub use device::DeviceInfo;
pub use error::{Error, ErrorCode, Result};
pub use frame::{Frame, FrameGuard, FrameInfo};
pub use grabbing::{CallbackGrabbing, Grabbing};
pub use kind::{AccessMode, PixelType, TransportLayer};
pub use sdk::Sdk;

/// 读取 SDK 定长字符数组中首个 NUL 之前的字符串。
///
/// 厂商保证这些字段以 NUL 结尾；缺少 NUL 时返回空串，避免越界读取。
fn fixed_cstr(bytes: &[u8]) -> &CStr {
    CStr::from_bytes_until_nul(bytes).unwrap_or_default()
}

/// [`fixed_cstr`] 的 `c_char` 版本。
fn fixed_cstr_from_chars(chars: &[c_char]) -> &CStr {
    // SAFETY: `c_char` 与 `u8` 大小、对齐相同，只重新解释已初始化的字节。
    let bytes = unsafe { std::slice::from_raw_parts(chars.as_ptr().cast::<u8>(), chars.len()) };
    fixed_cstr(bytes)
}

/// 合并 SDK 拆成高低两半的 64 位值。
fn high_low(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}
