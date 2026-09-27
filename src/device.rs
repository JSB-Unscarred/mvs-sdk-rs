//! 枚举得到的设备信息。

use std::ffi::CStr;
use std::fmt;
use std::net::Ipv4Addr;

use crate::{TransportLayer, fixed_cstr, sys};

/// `MV_CC_DEVICE_INFO` 的拥有副本。
///
/// 字符串字段按 transport 从对应的 union 成员读取，该 transport 没有的字段返回空串。
/// 本值不持有 SDK 会话；打开设备见 [`Sdk::open`](crate::Sdk::open)。
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
    pub(crate) const fn from_raw(raw: &sys::MV_CC_DEVICE_INFO) -> Self {
        Self { raw: *raw }
    }

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

    fn string(&self, field: impl FnOnce(Strings<'_>) -> &[u8]) -> &CStr {
        self.strings()
            .map_or(c"", |strings| fixed_cstr(field(strings)))
    }

    fn gige(&self) -> Option<&sys::MV_GIGE_DEVICE_INFO> {
        match self.raw.nTLayerType {
            sys::MV_GIGE_DEVICE | sys::MV_VIR_GIGE_DEVICE | sys::MV_GENTL_GIGE_DEVICE => {
                // SAFETY: nTLayerType 指明 stGigEInfo 是有效的 union 成员。
                Some(unsafe { &self.raw.SpecialInfo.stGigEInfo })
            }
            _ => None,
        }
    }

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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::DeviceInfo;
    use crate::sys;

    // 字符串按 transport 选择 union 成员并截断到 NUL；非 GigE 设备没有 IP；MAC 丢弃高位字段的高 16 位。
    #[test]
    fn fields_follow_the_transport_layer() {
        let mut raw = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_GIGE_DEVICE,
            nMacAddrHigh: 0xFFFF_0011,
            nMacAddrLow: 0x2233_4455,
            ..Default::default()
        };
        // SAFETY: 测试只写入 stGigEInfo 成员，union 其余字节保持为零。
        unsafe {
            raw.SpecialInfo.stGigEInfo.chSerialNumber[..3].copy_from_slice(b"SN1");
            raw.SpecialInfo.stGigEInfo.nCurrentIp = 0xC0A8_0102;
        }

        let gige = DeviceInfo::from_raw(&raw);
        assert_eq!(gige.serial_number(), c"SN1");
        assert_eq!(gige.current_ip(), Some(Ipv4Addr::new(192, 168, 1, 2)));
        assert_eq!(gige.mac_address(), [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);

        raw.nTLayerType = sys::MV_CAMERALINK_DEVICE;
        let camera_link = DeviceInfo::from_raw(&raw);
        assert_eq!(camera_link.user_defined_name(), c"");
        assert_eq!(camera_link.current_ip(), None);
    }
}
