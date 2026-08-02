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

/// Error returned by [`Sdk::shutdown`](crate::Sdk::shutdown).
#[non_exhaustive]
#[derive(Debug)]
pub enum ShutdownError {
    /// Native camera resources or callbacks are still live.
    InUse {
        /// Cameras that have not completed handle destruction.
        live_cameras: usize,
        /// Callbacks that are still executing.
        active_callbacks: usize,
    },
    /// A native handle could not be destroyed safely, so finalization is
    /// permanently blocked for this process.
    UnresolvedResources {
        /// Number of native handles whose destruction could not be confirmed.
        orphaned_handles: usize,
    },
    /// The vendor finalization call failed.
    Finalize(MvsError),
    /// A previous finalization attempt left the vendor SDK in an unknown
    /// process-wide state. The call is not retried.
    StateUnknown {
        /// Original vendor error code, when one was available.
        finalize_code: Option<u32>,
    },
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse {
                live_cameras,
                active_callbacks,
            } => write!(
                f,
                "MVS SDK is still in use by {live_cameras} camera(s) and {active_callbacks} callback(s)"
            ),
            Self::UnresolvedResources { orphaned_handles } => write!(
                f,
                "MVS SDK cannot be finalized because {orphaned_handles} native handle(s) could not be destroyed"
            ),
            Self::Finalize(error) => write!(f, "MVS SDK finalization failed: {error}"),
            Self::StateUnknown {
                finalize_code: Some(code),
            } => write!(
                f,
                "MVS SDK state is unknown after finalization failed with 0x{code:08X}"
            ),
            Self::StateUnknown {
                finalize_code: None,
            } => f.write_str("MVS SDK state is unknown after finalization failed"),
        }
    }
}

impl std::error::Error for ShutdownError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Finalize(error) => Some(error),
            _ => None,
        }
    }
}

/// One internal operation attempted while closing a camera.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CleanupStep {
    /// Drain Rust callbacks before native teardown.
    DrainCallbacks,
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
            Self::DrainCallbacks => "drain callbacks",
            Self::StopGrabbing => "stop grabbing",
            Self::UnregisterImageCallback => "unregister image callback",
            Self::UnregisterExceptionCallback => "unregister exception callback",
            Self::UnregisterEventCallback => "unregister event callback",
            Self::CloseDevice => "close device",
            Self::DestroyHandle => "destroy handle",
        })
    }
}

/// One failed internal operation from a camera cleanup attempt.
#[non_exhaustive]
#[derive(Debug)]
pub(crate) struct CleanupFailure {
    /// The cleanup operation that failed.
    pub(crate) step: CleanupStep,
    /// The error returned by that operation.
    pub(crate) error: MvsError,
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

/// Errors returned by [`Camera::close_detailed`](crate::Camera::close_detailed).
///
/// Cleanup normally continues after each failure so that handle destruction
/// is attempted, and errors are retained in call order. An error therefore
/// does not imply that the native handle is still alive; use
/// [`CleanupError::native_handle_destroyed`] when that distinction matters.
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

    #[cfg(all(test, target_os = "windows", target_arch = "x86_64"))]
    pub(crate) fn failures(&self) -> &[CleanupFailure] {
        &self.failures
    }

    /// Return the non-empty cleanup errors in attempted call order.
    pub fn errors(&self) -> impl ExactSizeIterator<Item = &MvsError> {
        self.failures.iter().map(|failure| &failure.error)
    }

    pub(crate) fn into_first_error(self) -> MvsError {
        self.failures
            .into_iter()
            .next()
            .expect("cleanup errors are never empty")
            .error
    }

    /// Whether native handle destruction was confirmed despite the reported
    /// cleanup failures.
    pub fn native_handle_destroyed(&self) -> bool {
        !self.failures.iter().any(|failure| {
            matches!(
                failure.step,
                CleanupStep::DrainCallbacks | CleanupStep::DestroyHandle
            )
        })
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

/// Error returned by any MVS SDK call, plus Rust-side marshalling, lifecycle,
/// and open-rollback failures.
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
    ImageDecodec,
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

    /// The native MVS SDK backend is unavailable on this target.
    #[error("MVS SDK is only available on Windows x86_64")]
    UnsupportedPlatform,

    /// An SDK operation was requested before [`Sdk::init`](crate::Sdk::init).
    #[error("MVS SDK has not been initialized")]
    SdkNotInitialized,
    /// The process-wide SDK has already been finalized.
    #[error("MVS SDK has already been shut down for this process")]
    SdkFinalized,
    /// A failed finalization left the process-wide SDK state unknown.
    #[error("MVS SDK process state is unknown after finalization failed")]
    SdkStateUnknown,

    /// Opening failed and destroying the partially created handle also failed.
    #[error("camera open failed ({open}); rollback handle destruction also failed ({destroy})")]
    OpenRollback {
        /// Error returned while opening the device.
        open: Box<MvsError>,
        /// Error returned while rolling back the newly created handle.
        destroy: Box<MvsError>,
    },
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
            Self::Nul(_)
            | Self::UnsupportedPlatform
            | Self::SdkNotInitialized
            | Self::SdkFinalized
            | Self::SdkStateUnknown
            | Self::OpenRollback { .. } => return None,
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
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    use super::{CleanupError, CleanupFailure, CleanupStep};
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

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn cleanup_error_preserves_order_and_selects_the_first_error() {
        let error = CleanupError::new(vec![
            CleanupFailure {
                step: CleanupStep::StopGrabbing,
                error: MvsError::Resource,
            },
            CleanupFailure {
                step: CleanupStep::CloseDevice,
                error: MvsError::CallOrder,
            },
        ]);

        assert_eq!(
            error.errors().map(MvsError::raw_code).collect::<Vec<_>>(),
            [Some(sys::MV_E_RESOURCE), Some(sys::MV_E_CALLORDER)]
        );
        assert!(error.native_handle_destroyed());
        assert_eq!(
            error.into_first_error().raw_code(),
            Some(sys::MV_E_RESOURCE)
        );

        let destroy_error = CleanupError::new(vec![CleanupFailure {
            step: CleanupStep::DestroyHandle,
            error: MvsError::Resource,
        }]);
        assert!(!destroy_error.native_handle_destroyed());
    }
}
