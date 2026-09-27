//! 错误类型与 SDK 状态码。

use std::ffi::c_int;
use std::fmt;

use crate::sys;

/// 本 crate 的 `Result` 别名。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// 本 crate 的错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// SDK 函数返回了非 `MV_OK` 的状态码。
    #[error("{function} failed: {code}")]
    Sdk {
        /// 失败的 SDK 函数名。
        function: &'static str,
        /// SDK 返回的状态码。
        code: ErrorCode,
    },
    /// 本进程的 SDK 会话已经 `MV_CC_Finalize`；厂商约定每个进程只初始化一次，不能再次初始化。
    #[error("MVS SDK was finalized in this process and cannot be initialized again")]
    Finalized,
}

/// 把 SDK 返回值转换为 `Result`；通常经由 [`sdk_call!`] 调用。
pub(crate) fn check(function: &'static str, code: c_int) -> Result<()> {
    let code = code.cast_unsigned();
    if code == sys::MV_OK {
        Ok(())
    } else {
        Err(Error::Sdk {
            function,
            code: ErrorCode::from_raw(code),
        })
    }
}

/// 调用返回状态码的 SDK 函数，失败时生成带函数名的 [`Error::Sdk`]。
///
/// 宏本身不含 `unsafe`，调用点仍须位于带 SAFETY 注释的 `unsafe` 块中。
macro_rules! sdk_call {
    ($function:ident($($argument:expr),* $(,)?)) => {
        $crate::error::check(stringify!($function), $crate::sys::$function($($argument),*))
    };
}
pub(crate) use sdk_call;

macro_rules! error_codes {
    ($($(#[$meta:meta])* $variant:ident = $code:ident,)+) => {
        /// `MvErrorDefine.h` 中的状态码；头文件未定义的值保存在 [`ErrorCode::Other`]。
        ///
        /// `Display` 输出头文件中的宏名与十六进制值，便于对照厂商文档。
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $($(#[$meta])* $variant,)+
            /// 头文件未定义的状态码。
            Other(u32),
        }

        impl ErrorCode {
            /// 由 SDK 返回值构造。
            pub const fn from_raw(code: u32) -> Self {
                match code {
                    $(sys::$code => Self::$variant,)+
                    other => Self::Other(other),
                }
            }

            /// 返回 SDK 状态码。
            pub const fn raw(self) -> u32 {
                match self {
                    $(Self::$variant => sys::$code,)+
                    Self::Other(code) => code,
                }
            }

            /// 头文件中的宏名。
            const fn name(self) -> Option<&'static str> {
                match self {
                    $(Self::$variant => Some(stringify!($code)),)+
                    Self::Other(_) => None,
                }
            }
        }
    };
}

error_codes! {
    /// 错误或无效的句柄。
    Handle = MV_E_HANDLE,
    /// 不支持的功能。
    NotSupported = MV_E_SUPPORT,
    /// 缓存已满。
    BufferOverflow = MV_E_BUFOVER,
    /// 函数调用顺序错误。
    CallOrder = MV_E_CALLORDER,
    /// 参数错误。
    Parameter = MV_E_PARAMETER,
    /// 资源申请失败。
    Resource = MV_E_RESOURCE,
    /// 无数据，例如取图超时。
    NoData = MV_E_NODATA,
    /// 前置条件有误或运行环境已变化。
    Precondition = MV_E_PRECONDITION,
    /// 版本不匹配。
    Version = MV_E_VERSION,
    /// 传入的内存空间不足。
    NotEnoughBuffer = MV_E_NOENOUGH_BUF,
    /// 异常图像，可能因丢包而不完整。
    AbnormalImage = MV_E_ABNORMAL_IMAGE,
    /// 动态库加载失败。
    LoadLibrary = MV_E_LOAD_LIBRARY,
    /// 没有可输出的缓存。
    NoOutputBuffer = MV_E_NOOUTBUF,
    /// 加密错误。
    Encrypt = MV_E_ENCRYPT,
    /// 打开文件失败。
    OpenFile = MV_E_OPENFILE,
    /// 缓存地址已被使用。
    BufferInUse = MV_E_BUF_IN_USE,
    /// 无效的缓存地址。
    BufferInvalid = MV_E_BUF_INVALID,
    /// 缓存对齐异常。
    NoAlignBuffer = MV_E_NOALIGN_BUF,
    /// 缓存个数不足。
    NotEnoughBufferNum = MV_E_NOENOUGH_BUF_NUM,
    /// 串口被占用。
    PortInUse = MV_E_PORT_IN_USE,
    /// 图像解码失败。
    ImageDecode = MV_E_IMAGE_DECODEC,
    /// 图像大小超过 `u32` 范围。
    Uint32Limit = MV_E_UINT32_LIMIT,
    /// 图像高度异常。
    ImageHeight = MV_E_IMAGE_HEIGHT,
    /// 设备 DDR 缓存不足。
    NotEnoughDdr = MV_E_NOENOUGH_DDR,
    /// 流通道不足。
    NotEnoughStream = MV_E_NOENOUGH_STREAM,
    /// 设备无响应。
    NoResponse = MV_E_NORESPONSE,
    /// 未知错误。
    Unknown = MV_E_UNKNOW,
    /// `GenICam` 通用错误。
    GcGeneric = MV_E_GC_GENERIC,
    /// `GenICam` 参数非法。
    GcArgument = MV_E_GC_ARGUMENT,
    /// `GenICam` 值超出范围。
    GcRange = MV_E_GC_RANGE,
    /// `GenICam` 属性错误。
    GcProperty = MV_E_GC_PROPERTY,
    /// `GenICam` 运行环境错误。
    GcRuntime = MV_E_GC_RUNTIME,
    /// `GenICam` 逻辑错误。
    GcLogical = MV_E_GC_LOGICAL,
    /// `GenICam` 节点当前不可访问。
    GcAccess = MV_E_GC_ACCESS,
    /// `GenICam` 超时。
    GcTimeout = MV_E_GC_TIMEOUT,
    /// `GenICam` 类型转换失败。
    GcDynamicCast = MV_E_GC_DYNAMICCAST,
    /// `GenICam` 未知错误。
    GcUnknown = MV_E_GC_UNKNOW,
    /// 设备不支持该命令。
    NotImplemented = MV_E_NOT_IMPLEMENTED,
    /// 访问的地址不存在。
    InvalidAddress = MV_E_INVALID_ADDRESS,
    /// 地址不可写。
    WriteProtect = MV_E_WRITE_PROTECT,
    /// 无访问权限。
    AccessDenied = MV_E_ACCESS_DENIED,
    /// 设备忙或网络断开。
    Busy = MV_E_BUSY,
    /// 网络包错误。
    Packet = MV_E_PACKET,
    /// 网络错误。
    Net = MV_E_NETER,
    /// 当前模式不支持修改设备 IP。
    ModifyDeviceIpNotSupported = MV_E_SUPPORT_MODIFY_DEVICE_IP,
    /// 密钥校验失败。
    KeyVerificationFailed = MV_E_KEY_VERIFICATION,
    /// 设备 IP 冲突。
    IpConflict = MV_E_IP_CONFLICT,
    /// USB 读错误。
    UsbRead = MV_E_USB_READ,
    /// USB 写错误。
    UsbWrite = MV_E_USB_WRITE,
    /// USB 设备异常。
    UsbDevice = MV_E_USB_DEVICE,
    /// USB `GenICam` 错误。
    UsbGenicam = MV_E_USB_GENICAM,
    /// USB 带宽不足。
    UsbBandwidth = MV_E_USB_BANDWIDTH,
    /// USB 驱动不匹配或未安装。
    UsbDriver = MV_E_USB_DRIVER,
    /// USB 未知错误。
    UsbUnknown = MV_E_USB_UNKNOW,
    /// 固件与设备不匹配。
    UpgFileMismatch = MV_E_UPG_FILE_MISMATCH,
    /// 固件语言不匹配。
    UpgLanguageMismatch = MV_E_UPG_LANGUSGE_MISMATCH,
    /// 升级冲突。
    UpgConflict = MV_E_UPG_CONFLICT,
    /// 升级时设备内部错误。
    UpgInnerErr = MV_E_UPG_INNER_ERR,
    /// 升级未知错误。
    UpgUnknown = MV_E_UPG_UNKNOW,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (0x{:08X})", self.raw()),
            None => write!(f, "unknown status code 0x{:08X}", self.raw()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, ErrorCode, check};
    use crate::sys;

    // 已知状态码映射到变体，未知状态码原样保留，错误信息带上失败的函数名。
    #[test]
    fn status_codes_round_trip_and_name_the_function() {
        assert_eq!(
            ErrorCode::from_raw(sys::MV_E_CALLORDER),
            ErrorCode::CallOrder
        );
        assert_eq!(ErrorCode::from_raw(0xDEAD_BEEF).raw(), 0xDEAD_BEEF);

        let error = check("MV_CC_StartGrabbing", sys::MV_E_CALLORDER.cast_signed()).unwrap_err();
        assert!(matches!(
            error,
            Error::Sdk {
                code: ErrorCode::CallOrder,
                ..
            }
        ));
        assert_eq!(
            error.to_string(),
            "MV_CC_StartGrabbing failed: MV_E_CALLORDER (0x80000003)"
        );
    }
}
