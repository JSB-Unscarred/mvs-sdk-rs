//! 枚举得到的设备信息。

use std::ffi::CStr;
use std::fmt;
use std::net::Ipv4Addr;

use crate::{TransportLayer, fixed_cstr_bytes, sys};

/// 枚举得到的设备信息，`MV_CC_DEVICE_INFO` 的副本。
///
/// 该设备的 transport 没有的字符串字段返回空串。打开设备见 [`Sdk::open`](crate::Sdk::open)。
#[derive(Clone)]
pub struct DeviceInfo {
    raw: sys::MV_CC_DEVICE_INFO,
}

/// 各 transport 的 union 成员里公共字符串字段的位置。
struct Strings<'a> {
    manufacturer_name: &'a [u8],
    model_name: &'a [u8],
    serial_number: &'a [u8],
    device_version: &'a [u8],
    user_defined_name: &'a [u8],
}

/// 各 union 成员只有制造商字段名不同（`GenTL` 设备为 `chVendorName`），
/// 原生 Camera Link 设备没有用户自定义名称。
macro_rules! strings {
    ($info:expr, $manufacturer:ident) => {
        strings!($info, $manufacturer, &$info.chUserDefinedName)
    };
    ($info:expr, $manufacturer:ident, $user_defined_name:expr) => {
        Strings {
            manufacturer_name: &$info.$manufacturer,
            model_name: &$info.chModelName,
            serial_number: &$info.chSerialNumber,
            device_version: &$info.chDeviceVersion,
            user_defined_name: $user_defined_name,
        }
    };
}

impl DeviceInfo {
    /// 复制 SDK 设备记录。
    pub(crate) const fn from_raw(raw: &sys::MV_CC_DEVICE_INFO) -> Self {
        Self { raw: *raw }
    }

    /// 借出设备记录，供 `CreateHandle` 与 `IsDeviceAccessible` 使用。
    pub(crate) const fn raw(&self) -> &sys::MV_CC_DEVICE_INFO {
        &self.raw
    }

    /// 设备所在的 transport。
    pub const fn transport_layer(&self) -> TransportLayer {
        TransportLayer::from_raw(self.raw.nTLayerType)
    }

    /// 厂商定义的设备类型信息。
    pub const fn device_type_info(&self) -> u32 {
        self.raw.nDevTypeInfo
    }

    /// MAC 地址。
    ///
    /// 同 `GigE` Vision 引导寄存器：`nMacAddrHigh` 的低 16 位是前 2 字节，`nMacAddrLow` 是后 4 字节。
    pub const fn mac_address(&self) -> [u8; 6] {
        let [_, _, high0, high1] = self.raw.nMacAddrHigh.to_be_bytes();
        let [low0, low1, low2, low3] = self.raw.nMacAddrLow.to_be_bytes();
        [high0, high1, low0, low1, low2, low3]
    }

    /// 制造商名称。
    pub fn manufacturer_name(&self) -> &CStr {
        self.string(|strings| strings.manufacturer_name)
    }

    /// 型号名称。
    pub fn model_name(&self) -> &CStr {
        self.string(|strings| strings.model_name)
    }

    /// 序列号。
    pub fn serial_number(&self) -> &CStr {
        self.string(|strings| strings.serial_number)
    }

    /// 设备版本。
    pub fn device_version(&self) -> &CStr {
        self.string(|strings| strings.device_version)
    }

    /// 用户自定义名称；原生 Camera Link 设备没有该字段。
    ///
    /// 字节按设备写入时的编码保存，厂商示例按系统 ANSI 代码页（中文 Windows 上为 GBK）解码。
    pub fn user_defined_name(&self) -> &CStr {
        self.string(|strings| strings.user_defined_name)
    }

    /// `GigE` 设备的当前 IP；其它 transport 返回 `None`。
    pub fn current_ip(&self) -> Option<Ipv4Addr> {
        self.gige().map(|info| Ipv4Addr::from(info.nCurrentIp))
    }

    /// `GigE` 设备的子网掩码。
    pub fn subnet_mask(&self) -> Option<Ipv4Addr> {
        self.gige()
            .map(|info| Ipv4Addr::from(info.nCurrentSubNetMask))
    }

    /// `GigE` 设备的默认网关。
    pub fn default_gateway(&self) -> Option<Ipv4Addr> {
        self.gige().map(|info| Ipv4Addr::from(info.nDefultGateWay))
    }

    /// `GigE` 设备所连主机网口的 IP。
    pub fn host_ip(&self) -> Option<Ipv4Addr> {
        self.gige().map(|info| Ipv4Addr::from(info.nNetExport))
    }

    /// 指向内部 `MV_CC_DEVICE_INFO` 的指针，供尚未封装的 SDK 接口使用，只在本值存活期间有效。
    pub const fn as_raw(&self) -> *const sys::MV_CC_DEVICE_INFO {
        &raw const self.raw
    }

    /// 按 transport 读取一个公共字符串字段；没有该字段的 transport 返回空串。
    fn string(&self, field: impl FnOnce(Strings<'_>) -> &[u8]) -> &CStr {
        self.strings()
            .map_or(c"", |strings| fixed_cstr_bytes(field(strings)))
    }

    /// `GigE` 设备的 union 成员；其它 transport 返回 `None`。
    fn gige(&self) -> Option<&sys::MV_GIGE_DEVICE_INFO> {
        match self.raw.nTLayerType {
            sys::MV_GIGE_DEVICE | sys::MV_VIR_GIGE_DEVICE | sys::MV_GENTL_GIGE_DEVICE => {
                // SAFETY: nTLayerType 指明 stGigEInfo 是有效的 union 成员。
                Some(unsafe { &self.raw.SpecialInfo.stGigEInfo })
            }
            _ => None,
        }
    }

    /// 按 nTLayerType 定位各 union 成员中的公共字符串字段。
    fn strings(&self) -> Option<Strings<'_>> {
        let info = &self.raw.SpecialInfo;
        // SAFETY: 每个分支只读取 nTLayerType 对应的 union 成员。
        unsafe {
            Some(match self.raw.nTLayerType {
                sys::MV_GIGE_DEVICE | sys::MV_VIR_GIGE_DEVICE | sys::MV_GENTL_GIGE_DEVICE => {
                    strings!(info.stGigEInfo, chManufacturerName)
                }
                sys::MV_USB_DEVICE | sys::MV_VIR_USB_DEVICE => {
                    strings!(info.stUsb3VInfo, chManufacturerName)
                }
                sys::MV_CAMERALINK_DEVICE => strings!(info.stCamLInfo, chManufacturerName, &[]),
                sys::MV_GENTL_CAMERALINK_DEVICE => strings!(info.stCMLInfo, chVendorName),
                sys::MV_GENTL_CXP_DEVICE => strings!(info.stCXPInfo, chVendorName),
                sys::MV_GENTL_XOF_DEVICE => strings!(info.stXoFInfo, chVendorName),
                sys::MV_GENTL_VIR_DEVICE => strings!(info.stVirInfo, chVendorName),
                _ => return None,
            })
        }
    }
}

impl fmt::Debug for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInfo")
            .field("transport_layer", &self.transport_layer())
            .field("model_name", &self.model_name())
            .field("serial_number", &self.serial_number())
            .field("current_ip", &self.current_ip())
            .finish_non_exhaustive()
    }
}
