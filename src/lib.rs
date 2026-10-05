//! 海康机器人（Hikrobot）MVS 工业相机 SDK 的安全 Rust 封装（非官方）。
//!
//! 只支持 `x86_64-pc-windows-msvc`；编译不需要 MVS，运行时需要安装 MVS 4.7.0 或更新版本。
//!
//! - [`Sdk`]：本进程的 SDK 会话，枚举并打开设备；
//! - [`Camera`]：一台打开的相机，读写节点，注册 exception/event callback；
//! - [`Grabbing`] 与 [`CallbackGrabbing`]：主动取图或 callback 取图的守卫，借用或按值持有相机，
//!   释放时停止取流。
//!
//! 所有权、清理与 callback 的约定见 [`docs::architecture`]；未封装的 SDK 接口可以经 [`sys`] 调用。
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use mvs_sdk::{AccessMode, Sdk, TransportLayer};
//!
//! fn main() -> mvs_sdk::Result<()> {
//!     let sdk = Sdk::new()?;
//!     let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
//!     let mut camera = sdk.open(&devices[0], AccessMode::Exclusive, 0)?;
//!     camera.set_float(c"ExposureTime", 10_000.0)?;
//!
//!     let grabbing = camera.start_grabbing()?;
//!     let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
//!     let frame = buffer.frame();
//!     println!("{:?}, {} bytes", frame.info, frame.data.len());
//!     Ok(())
//! }
//! ```

use std::ffi::{CStr, CString, c_char};
use std::slice;

/// 原始 FFI（`mvs-sdk-sys`）；未封装的 SDK 接口配合 [`Camera::as_raw_handle`] 与 [`DeviceInfo::as_raw`] 调用。
pub use mvs_sdk_sys as sys;

mod callback;
mod camera;
mod device_info;
pub mod docs;
mod error;
mod frame;
mod grabbing;
mod kind;
mod sdk;

pub use callback::{EventInfo, ExceptionKind};
pub use camera::{Camera, EnumValue, FloatValue, IntValue, StringValue};
pub use device_info::DeviceInfo;
pub use error::{Error, ErrorCode, Result};
pub use frame::{Frame, FrameGuard, FrameInfo};
pub use grabbing::{CallbackGrabbing, Grabbing, HoldsCamera};
pub use kind::{AccessMode, PixelType, TransportLayer};
pub use sdk::Sdk;

/// 读取 SDK 定长字符数组中首个 NUL 之前的字符串。
///
/// 这些字段是 C 字符串，厂商示例直接以 `%s` 读取，写入方保证以 NUL 结尾；缺少 NUL 属于违约数据，
/// 此时返回空串而不越界读取。
fn fixed_cstr(chars: &[c_char]) -> &CStr {
    fixed_cstr_bytes(char_bytes(chars))
}

/// [`fixed_cstr`] 的字节版本：bindgen 把设备信息中的 `unsigned char` 字符串生成为 `u8` 数组。
fn fixed_cstr_bytes(bytes: &[u8]) -> &CStr {
    CStr::from_bytes_until_nul(bytes).unwrap_or_default()
}

/// 复制 SDK 定长字符数组中的字符串：截到首个 NUL，字段写满、没有 NUL 时取整个字段，不丢数据。
fn fixed_cstring(chars: &[c_char]) -> CString {
    let bytes = char_bytes(chars);
    let len = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    // 截到首个 NUL 后不含内部 NUL，CString::new 不会失败。
    CString::new(&bytes[..len]).unwrap_or_default()
}

/// 把 SDK 的 `c_char` 数组按字节读取。
fn char_bytes(chars: &[c_char]) -> &[u8] {
    // SAFETY: `c_char` 与 `u8` 大小、对齐相同，只重新解释已初始化的字节。
    unsafe { slice::from_raw_parts(chars.as_ptr().cast::<u8>(), chars.len()) }
}

/// 合并 SDK 拆成高低两半的 64 位值。
fn high_low(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

// 两份 README 中的 Rust 示例参与 doctest，避免与 API 脱节。
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
#[doc = include_str!("../README.en.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use super::{fixed_cstr, fixed_cstring};

    // 拥有型字符串无损：字段写满、没有 NUL 时取整个字段；借用型按厂商约定截到 NUL，违约时为空串。
    #[test]
    fn fixed_strings_follow_the_nul_policy() {
        let full = [b'a'.cast_signed(); 4];
        assert_eq!(fixed_cstring(&full).as_bytes(), b"aaaa");
        assert_eq!(fixed_cstr(&full), c"");

        let terminated = [b'a'.cast_signed(), 0, b'b'.cast_signed(), 0];
        assert_eq!(fixed_cstring(&terminated).as_bytes(), b"a");
        assert_eq!(fixed_cstr(&terminated), c"a");
    }
}
