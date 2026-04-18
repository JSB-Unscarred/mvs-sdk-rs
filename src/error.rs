//! Error type for the MVS SDK.
//!
//! [`MvsError`] covers every code defined in `MvErrorDefine.h` plus Rust-side
//! conditions (interior NUL bytes, UTF-8 failures). Unknown codes are
//! preserved via [`MvsError::Unknown`] so nothing is lost.

use std::ffi::NulError;
use std::os::raw::c_int;
use std::str::Utf8Error;

use crate::sys;

/// Crate-wide result alias.
pub type MvsResult<T> = Result<T, MvsError>;

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
            Self::Nul(_) | Self::Utf8(_) => return None,
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
pub(crate) fn check(code: c_int) -> MvsResult<()> {
    if code as u32 == sys::MV_OK {
        Ok(())
    } else {
        Err(MvsError::from(code))
    }
}
