//! Error type for the MVS SDK.
//!
//! [`MvsError`] covers every code defined in `MvErrorDefine.h` plus Rust-side
//! conditions (interior NUL bytes, UTF-8 failures). Unknown codes are
//! preserved via [`MvsError::Unknown`] so nothing is lost.

use std::ffi::NulError;
use std::fmt;
use std::os::raw::c_int;
use std::str::Utf8Error;

use crate::sys;

/// Crate-wide result alias.
pub type MvsResult<T> = Result<T, MvsError>;

/// A native operation attempted while closing a camera.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupStep {
    /// Stop image acquisition when it may still be active.
    StopGrabbing,
    /// Unregister the image callback.
    UnregisterImageCallback,
    /// Unregister the device-exception callback.
    UnregisterExceptionCallback,
    /// Unregister one named event callback.
    UnregisterEventCallback,
    /// Close the device.
    CloseDevice,
    /// Destroy the native handle.
    DestroyHandle,
}

impl fmt::Display for CleanupStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StopGrabbing => "stop grabbing",
            Self::UnregisterImageCallback => "unregister image callback",
            Self::UnregisterExceptionCallback => "unregister exception callback",
            Self::UnregisterEventCallback => "unregister event callback",
            Self::CloseDevice => "close device",
            Self::DestroyHandle => "destroy handle",
        })
    }
}

/// One failed native operation from a camera cleanup attempt.
#[non_exhaustive]
#[derive(Debug)]
pub struct CleanupFailure {
    /// The cleanup operation that failed.
    pub step: CleanupStep,
    /// The error returned by that operation.
    pub error: MvsError,
}

impl fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.step, self.error)
    }
}

impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// All failures observed while closing one camera.
///
/// Cleanup continues after each failure so that handle destruction is always
/// attempted, and failures are retained in call order. An error therefore
/// does not imply that the handle is still alive: destruction may have
/// succeeded after an earlier step failed.
#[derive(Debug)]
pub struct CleanupError {
    failures: Vec<CleanupFailure>,
}

impl CleanupError {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    pub(crate) fn new(failures: Vec<CleanupFailure>) -> Self {
        debug_assert!(!failures.is_empty());
        Self { failures }
    }

    /// Return cleanup failures in the order the native calls were attempted.
    pub fn failures(&self) -> &[CleanupFailure] {
        &self.failures
    }

    /// Consume this error and return its ordered failures.
    pub fn into_failures(self) -> Vec<CleanupFailure> {
        self.failures
    }
}

impl fmt::Display for CleanupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "camera cleanup failed in {} step(s)",
            self.failures.len()
        )?;
        for failure in &self.failures {
            write!(f, "; {failure}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CleanupError {}

/// Error returned by any MVS SDK call, plus Rust-side failures that arise
/// while marshalling arguments.
#[derive(thiserror::Error, Debug)]
pub enum MvsError {
    // ---- Generic SDK errors (0x80000000 - 0x800000FF) ----
    #[error("invalid handle")]
    Handle,
    #[error("unsupported operation")]
    NotSupported,
    #[error("buffer overflow")]
    BufferOverflow,
    #[error("incorrect call order")]
    CallOrder,
    #[error("invalid parameter")]
    Parameter,
    #[error("resource allocation failed")]
    Resource,
    #[error("no data")]
    NoData,
    #[error("precondition failed or environment changed")]
    Precondition,
    #[error("version mismatch")]
    Version,
    #[error("insufficient memory")]
    NotEnoughBuffer,
    #[error("abnormal image (possibly incomplete due to packet loss)")]
    AbnormalImage,
    #[error("failed to load library")]
    LoadLibrary,
    #[error("no available output buffer")]
    NoOutputBuffer,
    #[error("encryption error")]
    Encrypt,
    #[error("open file failed")]
    OpenFile,
    #[error("buffer already in use")]
    BufferInUse,
    #[error("invalid buffer address")]
    BufferInvalid,
    #[error("buffer alignment error")]
    NoAlignBuffer,
    #[error("insufficient buffer count")]
    NotEnoughBufferNum,
    #[error("port in use")]
    PortInUse,
    #[error("image decoding error")]
    ImageDecodec,
    #[error("image size exceeds u32 limit")]
    Uint32Limit,
    #[error("image height anomaly")]
    ImageHeight,
    #[error("insufficient DDR cache")]
    NotEnoughDdr,
    #[error("insufficient stream channels")]
    NotEnoughStream,
    #[error("no response from device")]
    NoResponse,
    #[error("unknown generic error")]
    UnknownGeneric,

    // ---- GenICam errors (0x80000100 - 0x800001FF) ----
    #[error("GenICam: general error")]
    GcGeneric,
    #[error("GenICam: illegal argument")]
    GcArgument,
    #[error("GenICam: value out of range")]
    GcRange,
    #[error("GenICam: property error")]
    GcProperty,
    #[error("GenICam: runtime error")]
    GcRuntime,
    #[error("GenICam: logical error")]
    GcLogical,
    #[error("GenICam: node access condition error")]
    GcAccess,
    #[error("GenICam: timeout")]
    GcTimeout,
    #[error("GenICam: dynamic cast error")]
    GcDynamicCast,
    #[error("GenICam: unknown error")]
    GcUnknown,

    // ---- GigE errors (0x80000200 - 0x800002FF) ----
    #[error("GigE: command not implemented by device")]
    NotImplemented,
    #[error("GigE: invalid address")]
    InvalidAddress,
    #[error("GigE: write protected")]
    WriteProtect,
    #[error("GigE: access denied")]
    AccessDenied,
    #[error("GigE: device busy or network disconnected")]
    Busy,
    #[error("GigE: network packet error")]
    Packet,
    #[error("GigE: network error")]
    Net,
    #[error("GigE: modifying the device IP is not supported")]
    ModifyDeviceIpNotSupported,
    #[error("GigE: key verification failed")]
    KeyVerificationFailed,
    #[error("GigE: device IP conflict")]
    IpConflict,

    // ---- USB errors (0x80000300 - 0x800003FF) ----
    #[error("USB: read error")]
    UsbRead,
    #[error("USB: write error")]
    UsbWrite,
    #[error("USB: device exception")]
    UsbDevice,
    #[error("USB: GenICam error")]
    UsbGenicam,
    #[error("USB: insufficient bandwidth")]
    UsbBandwidth,
    #[error("USB: driver mismatch or missing")]
    UsbDriver,
    #[error("USB: unknown error")]
    UsbUnknown,

    // ---- Upgrade errors (0x80000400 - 0x800004FF) ----
    #[error("upgrade: firmware mismatch")]
    UpgFileMismatch,
    #[error("upgrade: firmware language mismatch")]
    UpgLanguageMismatch,
    #[error("upgrade: conflict (already upgrading)")]
    UpgConflict,
    #[error("upgrade: internal device error")]
    UpgInnerErr,
    #[error("upgrade: unknown error")]
    UpgUnknown,

    // ---- Unknown SDK code ----
    #[error("unknown MVS error code: 0x{0:08X}")]
    Unknown(u32),

    // ---- Rust-side failures ----
    #[error("string contains interior NUL byte: {0}")]
    Nul(#[from] NulError),
    #[error("SDK returned non-UTF-8 data: {0}")]
    Utf8(#[from] Utf8Error),

    #[error("MVS SDK is only available on Windows x86_64")]
    UnsupportedPlatform,
}

impl MvsError {
    /// Return the raw SDK return code, if this error originated from the SDK.
    pub fn raw_code(&self) -> Option<u32> {
        let code = match self {
            Self::Handle => sys::MV_E_HANDLE,
            Self::NotSupported => sys::MV_E_SUPPORT,
            Self::BufferOverflow => sys::MV_E_BUFOVER,
            Self::CallOrder => sys::MV_E_CALLORDER,
            Self::Parameter => sys::MV_E_PARAMETER,
            Self::Resource => sys::MV_E_RESOURCE,
            Self::NoData => sys::MV_E_NODATA,
            Self::Precondition => sys::MV_E_PRECONDITION,
            Self::Version => sys::MV_E_VERSION,
            Self::NotEnoughBuffer => sys::MV_E_NOENOUGH_BUF,
            Self::AbnormalImage => sys::MV_E_ABNORMAL_IMAGE,
            Self::LoadLibrary => sys::MV_E_LOAD_LIBRARY,
            Self::NoOutputBuffer => sys::MV_E_NOOUTBUF,
            Self::Encrypt => sys::MV_E_ENCRYPT,
            Self::OpenFile => sys::MV_E_OPENFILE,
            Self::BufferInUse => sys::MV_E_BUF_IN_USE,
            Self::BufferInvalid => sys::MV_E_BUF_INVALID,
            Self::NoAlignBuffer => sys::MV_E_NOALIGN_BUF,
            Self::NotEnoughBufferNum => sys::MV_E_NOENOUGH_BUF_NUM,
            Self::PortInUse => sys::MV_E_PORT_IN_USE,
            Self::ImageDecodec => sys::MV_E_IMAGE_DECODEC,
            Self::Uint32Limit => sys::MV_E_UINT32_LIMIT,
            Self::ImageHeight => sys::MV_E_IMAGE_HEIGHT,
            Self::NotEnoughDdr => sys::MV_E_NOENOUGH_DDR,
            Self::NotEnoughStream => sys::MV_E_NOENOUGH_STREAM,
            Self::NoResponse => sys::MV_E_NORESPONSE,
            Self::UnknownGeneric => sys::MV_E_UNKNOW,
            Self::GcGeneric => sys::MV_E_GC_GENERIC,
            Self::GcArgument => sys::MV_E_GC_ARGUMENT,
            Self::GcRange => sys::MV_E_GC_RANGE,
            Self::GcProperty => sys::MV_E_GC_PROPERTY,
            Self::GcRuntime => sys::MV_E_GC_RUNTIME,
            Self::GcLogical => sys::MV_E_GC_LOGICAL,
            Self::GcAccess => sys::MV_E_GC_ACCESS,
            Self::GcTimeout => sys::MV_E_GC_TIMEOUT,
            Self::GcDynamicCast => sys::MV_E_GC_DYNAMICCAST,
            Self::GcUnknown => sys::MV_E_GC_UNKNOW,
            Self::NotImplemented => sys::MV_E_NOT_IMPLEMENTED,
            Self::InvalidAddress => sys::MV_E_INVALID_ADDRESS,
            Self::WriteProtect => sys::MV_E_WRITE_PROTECT,
            Self::AccessDenied => sys::MV_E_ACCESS_DENIED,
            Self::Busy => sys::MV_E_BUSY,
            Self::Packet => sys::MV_E_PACKET,
            Self::Net => sys::MV_E_NETER,
            Self::ModifyDeviceIpNotSupported => sys::MV_E_SUPPORT_MODIFY_DEVICE_IP,
            Self::KeyVerificationFailed => sys::MV_E_KEY_VERIFICATION,
            Self::IpConflict => sys::MV_E_IP_CONFLICT,
            Self::UsbRead => sys::MV_E_USB_READ,
            Self::UsbWrite => sys::MV_E_USB_WRITE,
            Self::UsbDevice => sys::MV_E_USB_DEVICE,
            Self::UsbGenicam => sys::MV_E_USB_GENICAM,
            Self::UsbBandwidth => sys::MV_E_USB_BANDWIDTH,
            Self::UsbDriver => sys::MV_E_USB_DRIVER,
            Self::UsbUnknown => sys::MV_E_USB_UNKNOW,
            Self::UpgFileMismatch => sys::MV_E_UPG_FILE_MISMATCH,
            Self::UpgLanguageMismatch => sys::MV_E_UPG_LANGUSGE_MISMATCH,
            Self::UpgConflict => sys::MV_E_UPG_CONFLICT,
            Self::UpgInnerErr => sys::MV_E_UPG_INNER_ERR,
            Self::UpgUnknown => sys::MV_E_UPG_UNKNOW,
            Self::Unknown(c) => *c,
            Self::Nul(_) | Self::Utf8(_) | Self::UnsupportedPlatform => return None,
        };
        Some(code)
    }
}

impl From<c_int> for MvsError {
    fn from(code: c_int) -> Self {
        // Error constants come from bindgen as u32 (values > 0x7FFFFFFF).
        // SDK function returns are c_int (i32). Compare with matching bit
        // pattern via u32.
        match code as u32 {
            sys::MV_E_HANDLE => Self::Handle,
            sys::MV_E_SUPPORT => Self::NotSupported,
            sys::MV_E_BUFOVER => Self::BufferOverflow,
            sys::MV_E_CALLORDER => Self::CallOrder,
            sys::MV_E_PARAMETER => Self::Parameter,
            sys::MV_E_RESOURCE => Self::Resource,
            sys::MV_E_NODATA => Self::NoData,
            sys::MV_E_PRECONDITION => Self::Precondition,
            sys::MV_E_VERSION => Self::Version,
            sys::MV_E_NOENOUGH_BUF => Self::NotEnoughBuffer,
            sys::MV_E_ABNORMAL_IMAGE => Self::AbnormalImage,
            sys::MV_E_LOAD_LIBRARY => Self::LoadLibrary,
            sys::MV_E_NOOUTBUF => Self::NoOutputBuffer,
            sys::MV_E_ENCRYPT => Self::Encrypt,
            sys::MV_E_OPENFILE => Self::OpenFile,
            sys::MV_E_BUF_IN_USE => Self::BufferInUse,
            sys::MV_E_BUF_INVALID => Self::BufferInvalid,
            sys::MV_E_NOALIGN_BUF => Self::NoAlignBuffer,
            sys::MV_E_NOENOUGH_BUF_NUM => Self::NotEnoughBufferNum,
            sys::MV_E_PORT_IN_USE => Self::PortInUse,
            sys::MV_E_IMAGE_DECODEC => Self::ImageDecodec,
            sys::MV_E_UINT32_LIMIT => Self::Uint32Limit,
            sys::MV_E_IMAGE_HEIGHT => Self::ImageHeight,
            sys::MV_E_NOENOUGH_DDR => Self::NotEnoughDdr,
            sys::MV_E_NOENOUGH_STREAM => Self::NotEnoughStream,
            sys::MV_E_NORESPONSE => Self::NoResponse,
            sys::MV_E_UNKNOW => Self::UnknownGeneric,
            sys::MV_E_GC_GENERIC => Self::GcGeneric,
            sys::MV_E_GC_ARGUMENT => Self::GcArgument,
            sys::MV_E_GC_RANGE => Self::GcRange,
            sys::MV_E_GC_PROPERTY => Self::GcProperty,
            sys::MV_E_GC_RUNTIME => Self::GcRuntime,
            sys::MV_E_GC_LOGICAL => Self::GcLogical,
            sys::MV_E_GC_ACCESS => Self::GcAccess,
            sys::MV_E_GC_TIMEOUT => Self::GcTimeout,
            sys::MV_E_GC_DYNAMICCAST => Self::GcDynamicCast,
            sys::MV_E_GC_UNKNOW => Self::GcUnknown,
            sys::MV_E_NOT_IMPLEMENTED => Self::NotImplemented,
            sys::MV_E_INVALID_ADDRESS => Self::InvalidAddress,
            sys::MV_E_WRITE_PROTECT => Self::WriteProtect,
            sys::MV_E_ACCESS_DENIED => Self::AccessDenied,
            sys::MV_E_BUSY => Self::Busy,
            sys::MV_E_PACKET => Self::Packet,
            sys::MV_E_NETER => Self::Net,
            sys::MV_E_SUPPORT_MODIFY_DEVICE_IP => Self::ModifyDeviceIpNotSupported,
            sys::MV_E_KEY_VERIFICATION => Self::KeyVerificationFailed,
            sys::MV_E_IP_CONFLICT => Self::IpConflict,
            sys::MV_E_USB_READ => Self::UsbRead,
            sys::MV_E_USB_WRITE => Self::UsbWrite,
            sys::MV_E_USB_DEVICE => Self::UsbDevice,
            sys::MV_E_USB_GENICAM => Self::UsbGenicam,
            sys::MV_E_USB_BANDWIDTH => Self::UsbBandwidth,
            sys::MV_E_USB_DRIVER => Self::UsbDriver,
            sys::MV_E_USB_UNKNOW => Self::UsbUnknown,
            sys::MV_E_UPG_FILE_MISMATCH => Self::UpgFileMismatch,
            sys::MV_E_UPG_LANGUSGE_MISMATCH => Self::UpgLanguageMismatch,
            sys::MV_E_UPG_CONFLICT => Self::UpgConflict,
            sys::MV_E_UPG_INNER_ERR => Self::UpgInnerErr,
            sys::MV_E_UPG_UNKNOW => Self::UpgUnknown,
            other => Self::Unknown(other),
        }
    }
}

impl From<u32> for MvsError {
    fn from(code: u32) -> Self {
        Self::from(code as c_int)
    }
}

/// Convert an SDK return code to a `MvsResult<()>`.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn check(code: c_int) -> MvsResult<()> {
    if code as u32 == sys::MV_OK {
        Ok(())
    } else {
        Err(MvsError::from(code))
    }
}

#[cfg(test)]
mod tests {
    use super::MvsError;
    use crate::sys;

    #[test]
    fn every_known_sdk_error_round_trips() {
        let codes = [
            sys::MV_E_HANDLE,
            sys::MV_E_SUPPORT,
            sys::MV_E_BUFOVER,
            sys::MV_E_CALLORDER,
            sys::MV_E_PARAMETER,
            sys::MV_E_RESOURCE,
            sys::MV_E_NODATA,
            sys::MV_E_PRECONDITION,
            sys::MV_E_VERSION,
            sys::MV_E_NOENOUGH_BUF,
            sys::MV_E_ABNORMAL_IMAGE,
            sys::MV_E_LOAD_LIBRARY,
            sys::MV_E_NOOUTBUF,
            sys::MV_E_ENCRYPT,
            sys::MV_E_OPENFILE,
            sys::MV_E_BUF_IN_USE,
            sys::MV_E_BUF_INVALID,
            sys::MV_E_NOALIGN_BUF,
            sys::MV_E_NOENOUGH_BUF_NUM,
            sys::MV_E_PORT_IN_USE,
            sys::MV_E_IMAGE_DECODEC,
            sys::MV_E_UINT32_LIMIT,
            sys::MV_E_IMAGE_HEIGHT,
            sys::MV_E_NOENOUGH_DDR,
            sys::MV_E_NOENOUGH_STREAM,
            sys::MV_E_NORESPONSE,
            sys::MV_E_UNKNOW,
            sys::MV_E_GC_GENERIC,
            sys::MV_E_GC_ARGUMENT,
            sys::MV_E_GC_RANGE,
            sys::MV_E_GC_PROPERTY,
            sys::MV_E_GC_RUNTIME,
            sys::MV_E_GC_LOGICAL,
            sys::MV_E_GC_ACCESS,
            sys::MV_E_GC_TIMEOUT,
            sys::MV_E_GC_DYNAMICCAST,
            sys::MV_E_GC_UNKNOW,
            sys::MV_E_NOT_IMPLEMENTED,
            sys::MV_E_INVALID_ADDRESS,
            sys::MV_E_WRITE_PROTECT,
            sys::MV_E_ACCESS_DENIED,
            sys::MV_E_BUSY,
            sys::MV_E_PACKET,
            sys::MV_E_NETER,
            sys::MV_E_SUPPORT_MODIFY_DEVICE_IP,
            sys::MV_E_KEY_VERIFICATION,
            sys::MV_E_IP_CONFLICT,
            sys::MV_E_USB_READ,
            sys::MV_E_USB_WRITE,
            sys::MV_E_USB_DEVICE,
            sys::MV_E_USB_GENICAM,
            sys::MV_E_USB_BANDWIDTH,
            sys::MV_E_USB_DRIVER,
            sys::MV_E_USB_UNKNOW,
            sys::MV_E_UPG_FILE_MISMATCH,
            sys::MV_E_UPG_LANGUSGE_MISMATCH,
            sys::MV_E_UPG_CONFLICT,
            sys::MV_E_UPG_INNER_ERR,
            sys::MV_E_UPG_UNKNOW,
        ];

        for code in codes {
            let error = MvsError::from(code);
            assert!(
                !matches!(&error, MvsError::Unknown(_)),
                "known SDK code was not decoded: 0x{code:08X}"
            );
            assert_eq!(error.raw_code(), Some(code));
        }
    }

    #[test]
    fn unknown_and_platform_errors_preserve_their_origin() {
        let code = 0xDEAD_BEEF;
        assert_eq!(MvsError::from(code).raw_code(), Some(code));
        assert_eq!(MvsError::UnsupportedPlatform.raw_code(), None);
    }
}
