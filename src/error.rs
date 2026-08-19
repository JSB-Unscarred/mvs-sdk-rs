//! Error type for the MVS SDK.
//!
//! [`MvsError`] covers every code defined in `MvErrorDefine.h` plus Rust-side
//! marshalling and lifecycle failures. Unknown codes are preserved via
//! [`MvsError::Unknown`] so nothing is lost.

use std::ffi::NulError;
use std::fmt;
use std::os::raw::c_int;

use crate::sys;

/// Crate-wide result alias.
pub type MvsResult<T> = Result<T, MvsError>;

/// `Camera::close` 返回的清理结果。
///
/// owner 线程的清理会继续执行到 `DestroyHandle`，因此分别保留 Destroy 前的首个
/// 失败操作与错误，以及 Destroy 错误；callback 上下文则拒绝 native teardown。只有
/// [`CleanupError::native_handle_destroyed`] 为 `true` 时，native handle 才已确认失效。
#[derive(Debug)]
pub struct CleanupError {
    prior_error: Option<(&'static str, MvsError)>,
    destroy_error: Option<MvsError>,
    native_handle_destroyed: bool,
}

impl CleanupError {
    #[cfg(any(
        test,
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    ))]
    pub(crate) fn new(
        prior_error: Option<(&'static str, MvsError)>,
        destroy_error: Option<MvsError>,
        native_handle_destroyed: bool,
    ) -> Self {
        Self {
            prior_error,
            destroy_error,
            native_handle_destroyed,
        }
    }

    /// 返回 `DestroyHandle` 前首个失败操作的名称。
    pub fn prior_operation(&self) -> Option<&'static str> {
        self.prior_error.as_ref().map(|(operation, _)| *operation)
    }

    /// 返回 `DestroyHandle` 前遇到的首个错误。
    pub fn prior_error(&self) -> Option<&MvsError> {
        self.prior_error.as_ref().map(|(_, error)| error)
    }

    /// 返回独立保存的 `DestroyHandle` 错误。
    pub fn destroy_error(&self) -> Option<&MvsError> {
        self.destroy_error.as_ref()
    }

    /// 返回 native handle 是否已由 `DestroyHandle` 确认销毁。
    pub fn native_handle_destroyed(&self) -> bool {
        self.native_handle_destroyed
    }
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.prior_error, &self.destroy_error) {
            (Some((operation, prior)), Some(destroy)) => write!(
                f,
                "camera cleanup failed during {operation} ({prior}); DestroyHandle also failed ({destroy})"
            ),
            (Some((operation, prior)), None) => {
                write!(f, "camera cleanup failed during {operation}: {prior}")
            }
            (None, Some(destroy)) => write!(f, "DestroyHandle failed: {destroy}"),
            (None, None) => f.write_str("camera cleanup did not destroy the native handle"),
        }
    }
}

impl std::error::Error for CleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.prior_error()
            .map(|error| error as &(dyn std::error::Error + 'static))
            .or_else(|| {
                self.destroy_error()
                    .map(|error| error as &(dyn std::error::Error + 'static))
            })
    }
}

/// Error returned by any MVS SDK call, plus Rust-side marshalling and lifecycle
/// failures.
///
/// This enum is non-exhaustive so newer SDK releases and additional safe-layer
/// validation errors can be represented without another source-breaking change.
#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum MvsError {
    // ---- Generic SDK errors (0x80000000 - 0x800000FF) ----
    /// The native camera handle is invalid.
    #[error("invalid handle")]
    Handle,
    /// The device or SDK does not support the requested operation.
    #[error("unsupported operation")]
    NotSupported,
    /// A native buffer overflowed.
    #[error("buffer overflow")]
    BufferOverflow,
    /// The operation is invalid in the camera's current state.
    #[error("incorrect call order")]
    CallOrder,
    /// A parameter supplied to the SDK is invalid.
    #[error("invalid parameter")]
    Parameter,
    /// The SDK could not allocate a required resource.
    #[error("resource allocation failed")]
    Resource,
    /// No data is currently available.
    #[error("no data")]
    NoData,
    /// A precondition failed or the device environment changed.
    #[error("precondition failed or environment changed")]
    Precondition,
    /// The runtime and component versions are incompatible.
    #[error("version mismatch")]
    Version,
    /// The SDK has insufficient memory for the operation.
    #[error("insufficient memory")]
    NotEnoughBuffer,
    /// The image is abnormal, commonly because packets were lost.
    #[error("abnormal image (possibly incomplete due to packet loss)")]
    AbnormalImage,
    /// A required native library could not be loaded.
    #[error("failed to load library")]
    LoadLibrary,
    /// No output buffer is currently available.
    #[error("no available output buffer")]
    NoOutputBuffer,
    /// The SDK reported an encryption failure.
    #[error("encryption error")]
    Encrypt,
    /// The SDK could not open a required file.
    #[error("open file failed")]
    OpenFile,
    /// The requested buffer is already in use.
    #[error("buffer already in use")]
    BufferInUse,
    /// A buffer address is invalid.
    #[error("invalid buffer address")]
    BufferInvalid,
    /// A buffer does not meet the SDK's alignment requirements.
    #[error("buffer alignment error")]
    NoAlignBuffer,
    /// Too few buffers were configured for the operation.
    #[error("insufficient buffer count")]
    NotEnoughBufferNum,
    /// The requested port is already in use.
    #[error("port in use")]
    PortInUse,
    /// Image decoding failed.
    #[error("image decoding error")]
    ImageDecode,
    /// The image size exceeds the SDK's `u32` limit.
    #[error("image size exceeds u32 limit")]
    Uint32Limit,
    /// The image height reported by the device is invalid.
    #[error("image height anomaly")]
    ImageHeight,
    /// The device has insufficient DDR cache.
    #[error("insufficient DDR cache")]
    NotEnoughDdr,
    /// No additional stream channel is available.
    #[error("insufficient stream channels")]
    NotEnoughStream,
    /// The device did not respond.
    #[error("no response from device")]
    NoResponse,
    /// The SDK returned an unspecified generic error.
    #[error("unknown generic error")]
    UnknownGeneric,

    // ---- GenICam errors (0x80000100 - 0x800001FF) ----
    /// A general GenICam operation failed.
    #[error("GenICam: general error")]
    GcGeneric,
    /// A GenICam argument is invalid.
    #[error("GenICam: illegal argument")]
    GcArgument,
    /// A GenICam value is outside its accepted range.
    #[error("GenICam: value out of range")]
    GcRange,
    /// A GenICam property operation failed.
    #[error("GenICam: property error")]
    GcProperty,
    /// A GenICam runtime operation failed.
    #[error("GenICam: runtime error")]
    GcRuntime,
    /// A GenICam logical condition failed.
    #[error("GenICam: logical error")]
    GcLogical,
    /// The GenICam node is not accessible in its current state.
    #[error("GenICam: node access condition error")]
    GcAccess,
    /// A GenICam operation timed out.
    #[error("GenICam: timeout")]
    GcTimeout,
    /// A GenICam dynamic cast failed.
    #[error("GenICam: dynamic cast error")]
    GcDynamicCast,
    /// The SDK returned an unspecified GenICam error.
    #[error("GenICam: unknown error")]
    GcUnknown,

    // ---- GigE errors (0x80000200 - 0x800002FF) ----
    /// The GigE device does not implement the requested command.
    #[error("GigE: command not implemented by device")]
    NotImplemented,
    /// A GigE address is invalid.
    #[error("GigE: invalid address")]
    InvalidAddress,
    /// The addressed GigE register or property is write-protected.
    #[error("GigE: write protected")]
    WriteProtect,
    /// Access to the GigE device was denied.
    #[error("GigE: access denied")]
    AccessDenied,
    /// The GigE device is busy or disconnected from the network.
    #[error("GigE: device busy or network disconnected")]
    Busy,
    /// A GigE network packet was invalid or lost.
    #[error("GigE: network packet error")]
    Packet,
    /// A general GigE network operation failed.
    #[error("GigE: network error")]
    Net,
    /// This GigE device does not support changing its IP address.
    #[error("GigE: modifying the device IP is not supported")]
    ModifyDeviceIpNotSupported,
    /// GigE key verification failed.
    #[error("GigE: key verification failed")]
    KeyVerificationFailed,
    /// The GigE device's IP address conflicts with another host.
    #[error("GigE: device IP conflict")]
    IpConflict,

    // ---- USB errors (0x80000300 - 0x800003FF) ----
    /// Reading from the USB device failed.
    #[error("USB: read error")]
    UsbRead,
    /// Writing to the USB device failed.
    #[error("USB: write error")]
    UsbWrite,
    /// The USB device reported an exception.
    #[error("USB: device exception")]
    UsbDevice,
    /// A USB GenICam operation failed.
    #[error("USB: GenICam error")]
    UsbGenicam,
    /// The USB connection has insufficient bandwidth.
    #[error("USB: insufficient bandwidth")]
    UsbBandwidth,
    /// The USB driver is missing or incompatible.
    #[error("USB: driver mismatch or missing")]
    UsbDriver,
    /// The SDK returned an unspecified USB error.
    #[error("USB: unknown error")]
    UsbUnknown,

    // ---- Upgrade errors (0x80000400 - 0x800004FF) ----
    /// The firmware file does not match the device.
    #[error("upgrade: firmware mismatch")]
    UpgFileMismatch,
    /// The firmware language does not match the device.
    #[error("upgrade: firmware language mismatch")]
    UpgLanguageMismatch,
    /// A firmware upgrade is already in progress or conflicts with this one.
    #[error("upgrade: conflict (already upgrading)")]
    UpgConflict,
    /// The device reported an internal upgrade error.
    #[error("upgrade: internal device error")]
    UpgInnerErr,
    /// The SDK returned an unspecified upgrade error.
    #[error("upgrade: unknown error")]
    UpgUnknown,

    // ---- Unknown SDK code ----
    /// An unrecognized vendor error code, preserved without loss.
    #[error("unknown MVS error code: 0x{0:08X}")]
    Unknown(u32),

    // ---- Rust-side failures ----
    /// A Rust string passed to the C API contains an interior NUL byte.
    #[error("string contains interior NUL byte: {0}")]
    Nul(#[from] NulError),

    /// The operation conflicts with the safe wrapper's current state.
    #[error("invalid state: {0}")]
    InvalidState(&'static str),

    /// `CreateHandle` reported success without returning a handle.
    #[error("CreateHandle returned a null handle")]
    NullHandleAfterCreate,

    /// The native MVS SDK backend is unavailable on this target.
    #[error("MVS SDK is only available on Windows x86_64 MSVC")]
    UnsupportedPlatform,

    /// SDK finalization is blocked by a native handle whose owner was consumed without a
    /// confirmed DestroyHandle success.
    #[error("orphaned native camera handles are still live")]
    NativeHandlesLive,

    /// Creating or opening a camera failed and rollback destruction also failed.
    /// The handle is no longer recoverable through the safe API, so the host
    /// should treat this as a process-terminal cleanup failure.
    #[error("camera open failed ({open}); rollback DestroyHandle also failed ({destroy})")]
    OpenRollback {
        /// Original `CreateHandle` or `OpenDevice` error.
        #[source]
        open: Box<MvsError>,
        /// Error returned while destroying the partial handle.
        destroy: Box<MvsError>,
    },
}

// 公开 enum 保留完整 rustdoc，内部表只负责 native code 转换。
macro_rules! define_sdk_error_codes {
    ($($variant:ident => $code:path),+ $(,)?) => {
        impl MvsError {
            /// Return the raw SDK status code represented by a native error variant.
            pub fn raw_code(&self) -> Option<u32> {
                match self {
                    $(Self::$variant => Some($code),)+
                    Self::Unknown(code) => Some(*code),
                    Self::Nul(_)
                    | Self::InvalidState(_)
                    | Self::NullHandleAfterCreate
                    | Self::UnsupportedPlatform
                    | Self::NativeHandlesLive
                    | Self::OpenRollback { .. } => None,
                }
            }
        }

        impl From<c_int> for MvsError {
            fn from(code: c_int) -> Self {
                // Error constants come from bindgen as u32 (values above
                // i32::MAX). Compare the return value's matching bit pattern.
                match code as u32 {
                    $($code => Self::$variant,)+
                    other => Self::Unknown(other),
                }
            }
        }
    };
}

define_sdk_error_codes! {
    Handle => sys::MV_E_HANDLE,
    NotSupported => sys::MV_E_SUPPORT,
    BufferOverflow => sys::MV_E_BUFOVER,
    CallOrder => sys::MV_E_CALLORDER,
    Parameter => sys::MV_E_PARAMETER,
    Resource => sys::MV_E_RESOURCE,
    NoData => sys::MV_E_NODATA,
    Precondition => sys::MV_E_PRECONDITION,
    Version => sys::MV_E_VERSION,
    NotEnoughBuffer => sys::MV_E_NOENOUGH_BUF,
    AbnormalImage => sys::MV_E_ABNORMAL_IMAGE,
    LoadLibrary => sys::MV_E_LOAD_LIBRARY,
    NoOutputBuffer => sys::MV_E_NOOUTBUF,
    Encrypt => sys::MV_E_ENCRYPT,
    OpenFile => sys::MV_E_OPENFILE,
    BufferInUse => sys::MV_E_BUF_IN_USE,
    BufferInvalid => sys::MV_E_BUF_INVALID,
    NoAlignBuffer => sys::MV_E_NOALIGN_BUF,
    NotEnoughBufferNum => sys::MV_E_NOENOUGH_BUF_NUM,
    PortInUse => sys::MV_E_PORT_IN_USE,
    ImageDecode => sys::MV_E_IMAGE_DECODEC,
    Uint32Limit => sys::MV_E_UINT32_LIMIT,
    ImageHeight => sys::MV_E_IMAGE_HEIGHT,
    NotEnoughDdr => sys::MV_E_NOENOUGH_DDR,
    NotEnoughStream => sys::MV_E_NOENOUGH_STREAM,
    NoResponse => sys::MV_E_NORESPONSE,
    UnknownGeneric => sys::MV_E_UNKNOW,
    GcGeneric => sys::MV_E_GC_GENERIC,
    GcArgument => sys::MV_E_GC_ARGUMENT,
    GcRange => sys::MV_E_GC_RANGE,
    GcProperty => sys::MV_E_GC_PROPERTY,
    GcRuntime => sys::MV_E_GC_RUNTIME,
    GcLogical => sys::MV_E_GC_LOGICAL,
    GcAccess => sys::MV_E_GC_ACCESS,
    GcTimeout => sys::MV_E_GC_TIMEOUT,
    GcDynamicCast => sys::MV_E_GC_DYNAMICCAST,
    GcUnknown => sys::MV_E_GC_UNKNOW,
    NotImplemented => sys::MV_E_NOT_IMPLEMENTED,
    InvalidAddress => sys::MV_E_INVALID_ADDRESS,
    WriteProtect => sys::MV_E_WRITE_PROTECT,
    AccessDenied => sys::MV_E_ACCESS_DENIED,
    Busy => sys::MV_E_BUSY,
    Packet => sys::MV_E_PACKET,
    Net => sys::MV_E_NETER,
    ModifyDeviceIpNotSupported => sys::MV_E_SUPPORT_MODIFY_DEVICE_IP,
    KeyVerificationFailed => sys::MV_E_KEY_VERIFICATION,
    IpConflict => sys::MV_E_IP_CONFLICT,
    UsbRead => sys::MV_E_USB_READ,
    UsbWrite => sys::MV_E_USB_WRITE,
    UsbDevice => sys::MV_E_USB_DEVICE,
    UsbGenicam => sys::MV_E_USB_GENICAM,
    UsbBandwidth => sys::MV_E_USB_BANDWIDTH,
    UsbDriver => sys::MV_E_USB_DRIVER,
    UsbUnknown => sys::MV_E_USB_UNKNOW,
    UpgFileMismatch => sys::MV_E_UPG_FILE_MISMATCH,
    UpgLanguageMismatch => sys::MV_E_UPG_LANGUSGE_MISMATCH,
    UpgConflict => sys::MV_E_UPG_CONFLICT,
    UpgInnerErr => sys::MV_E_UPG_INNER_ERR,
    UpgUnknown => sys::MV_E_UPG_UNKNOW,
}

impl From<u32> for MvsError {
    fn from(code: u32) -> Self {
        Self::from(code as c_int)
    }
}

/// Convert an SDK return code to a `MvsResult<()>`.
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
pub(crate) fn check(code: c_int) -> MvsResult<()> {
    if code as u32 == sys::MV_OK {
        Ok(())
    } else {
        Err(MvsError::from(code))
    }
}

#[cfg(test)]
mod tests {
    use std::os::raw::c_int;

    use crate::sys;

    use super::{CleanupError, MvsError};

    // native 错误必须保留 variant 与原始返回码。
    #[test]
    fn known_native_code_is_mapped() {
        let error = MvsError::from(sys::MV_E_CALLORDER as c_int);

        assert!(matches!(&error, MvsError::CallOrder));
        assert_eq!(error.raw_code(), Some(sys::MV_E_CALLORDER));
    }

    // 核心错误约定：未知 native code 必须无损保留。
    #[test]
    fn unknown_sdk_code_is_preserved() {
        let code = 0xDEAD_BEEF;
        assert_eq!(MvsError::from(code).raw_code(), Some(code));
    }

    // safe wrapper 本地错误不得伪装成 native 返回码。
    #[test]
    fn local_errors_have_no_raw_code() {
        let errors = [
            MvsError::InvalidState("camera is already grabbing"),
            MvsError::NullHandleAfterCreate,
            MvsError::NativeHandlesLive,
        ];

        assert!(errors.iter().all(|error| error.raw_code().is_none()));
    }

    // 复合错误保留清理上下文，并暴露首个失败作为标准 error source。
    #[test]
    fn compound_errors_preserve_context_and_source() {
        let error = CleanupError::new(
            Some(("StopGrabbing", MvsError::CallOrder)),
            Some(MvsError::Handle),
            false,
        );

        assert_eq!(error.prior_operation(), Some("StopGrabbing"));
        assert!(matches!(error.prior_error(), Some(MvsError::CallOrder)));
        assert!(matches!(error.destroy_error(), Some(MvsError::Handle)));
        assert!(!error.native_handle_destroyed());
        assert!(error.to_string().contains("during StopGrabbing"));
        assert_eq!(
            std::error::Error::source(&error).and_then(|source| {
                source
                    .downcast_ref::<MvsError>()
                    .and_then(MvsError::raw_code)
            }),
            Some(sys::MV_E_CALLORDER)
        );

        let destroy_only = CleanupError::new(None, Some(MvsError::Handle), false);
        assert_eq!(
            std::error::Error::source(&destroy_only).and_then(|source| {
                source
                    .downcast_ref::<MvsError>()
                    .and_then(MvsError::raw_code)
            }),
            Some(sys::MV_E_HANDLE)
        );

        let rollback = MvsError::OpenRollback {
            open: Box::new(MvsError::Parameter),
            destroy: Box::new(MvsError::Handle),
        };
        assert_eq!(
            std::error::Error::source(&rollback).map(ToString::to_string),
            Some(MvsError::Parameter.to_string())
        );
    }
}
