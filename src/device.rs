//! Rust-owned 设备 metadata snapshot。

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::backend;
use crate::text::SdkText;
use crate::{AccessMode, TransportLayer};

/// backend 从 native record 抽出的公开字段。
pub(crate) struct DecodedDevice {
    pub major_version: u16,
    pub minor_version: u16,
    pub mac_address: [u8; 8],
    pub transport_layer: TransportLayer,
    pub device_type_info: u32,
    pub manufacturer: SdkText,
    pub model: SdkText,
    pub device_version: SdkText,
    pub manufacturer_specific_info: SdkText,
    pub serial: SdkText,
    pub user_defined_name: SdkText,
    pub current_ip: Option<Ipv4Addr>,
    pub current_subnet_mask: Option<Ipv4Addr>,
    pub default_gateway: Option<Ipv4Addr>,
    pub host_nic_ip: Option<Ipv4Addr>,
}

/// 从 SDK 枚举结果深拷贝得到的设备 snapshot。
///
/// 公开字段是 C 结构体中跨 transport 的常用子集，字符串保留原始字节。
/// 本值不持有 SDK session lease；打开与可访问性查询由 [`crate::Sdk`] 提供。
#[derive(Clone)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// `MV_CC_DEVICE_INFO::nMajorVer`。
    pub major_version: u16,
    /// `MV_CC_DEVICE_INFO::nMinorVer`。
    pub minor_version: u16,
    /// `nMacAddrHigh` 与 `nMacAddrLow` 按大端拼成 8 字节。
    pub mac_address: [u8; 8],
    /// `MV_CC_DEVICE_INFO::nTLayerType`。
    pub transport_layer: TransportLayer,
    /// `MV_CC_DEVICE_INFO::nDevTypeInfo`。
    pub device_type_info: u32,
    /// 制造商名称。
    pub manufacturer: SdkText,
    /// 型号名称。
    pub model: SdkText,
    /// 设备版本字符串；无对应字段时为空。
    pub device_version: SdkText,
    /// GigE 制造商附加信息；其它 transport 为空。
    pub manufacturer_specific_info: SdkText,
    /// 序列号。
    pub serial: SdkText,
    /// 用户自定义名称；无对应字段时为空。
    pub user_defined_name: SdkText,
    /// GigE 当前 IP；其它 transport 为 `None`。
    pub current_ip: Option<Ipv4Addr>,
    /// GigE 当前子网掩码；其它 transport 为 `None`。
    pub current_subnet_mask: Option<Ipv4Addr>,
    /// GigE 默认网关；其它 transport 为 `None`。
    pub default_gateway: Option<Ipv4Addr>,
    /// GigE 主机网口 IP；其它 transport 为 `None`。
    pub host_nic_ip: Option<Ipv4Addr>,
    inner: backend::DeviceInfo,
}

impl DeviceInfo {
    pub(crate) fn from_backend(inner: backend::DeviceInfo) -> Self {
        let decoded = inner.decode();
        Self {
            major_version: decoded.major_version,
            minor_version: decoded.minor_version,
            mac_address: decoded.mac_address,
            transport_layer: decoded.transport_layer,
            device_type_info: decoded.device_type_info,
            manufacturer: decoded.manufacturer,
            model: decoded.model,
            device_version: decoded.device_version,
            manufacturer_specific_info: decoded.manufacturer_specific_info,
            serial: decoded.serial,
            user_defined_name: decoded.user_defined_name,
            current_ip: decoded.current_ip,
            current_subnet_mask: decoded.current_subnet_mask,
            default_gateway: decoded.default_gateway,
            host_nic_ip: decoded.host_nic_ip,
            inner,
        }
    }

    pub(crate) fn clone_backend(&self) -> backend::DeviceInfo {
        self.inner.clone()
    }

    pub(crate) fn is_accessible(&self, mode: AccessMode) -> bool {
        self.inner.is_accessible(mode)
    }

    /// 返回设备是否使用 GigE transport。
    #[must_use]
    pub fn is_gige(&self) -> bool {
        self.transport_layer.contains(TransportLayer::GIGE)
            || self.transport_layer.contains(TransportLayer::VIR_GIGE)
            || self.transport_layer.contains(TransportLayer::GENTL_GIGE)
    }

    /// 返回设备是否使用 USB transport。
    #[must_use]
    pub fn is_usb(&self) -> bool {
        self.transport_layer.contains(TransportLayer::USB)
            || self.transport_layer.contains(TransportLayer::VIR_USB)
    }

    /// 借出当前 owned snapshot 的 opaque pointer。
    ///
    /// # Safety
    ///
    /// pointer 只在本值存活期间有效。通过 raw SDK 修改该记录属于 `unsafe` 操作。
    pub unsafe fn as_raw(&self) -> *const c_void {
        self.inner.as_raw()
    }
}

impl PartialEq for DeviceInfo {
    fn eq(&self, other: &Self) -> bool {
        self.major_version == other.major_version
            && self.minor_version == other.minor_version
            && self.mac_address == other.mac_address
            && self.transport_layer == other.transport_layer
            && self.device_type_info == other.device_type_info
            && self.manufacturer == other.manufacturer
            && self.model == other.model
            && self.device_version == other.device_version
            && self.manufacturer_specific_info == other.manufacturer_specific_info
            && self.serial == other.serial
            && self.user_defined_name == other.user_defined_name
            && self.current_ip == other.current_ip
            && self.current_subnet_mask == other.current_subnet_mask
            && self.default_gateway == other.default_gateway
            && self.host_nic_ip == other.host_nic_ip
    }
}

impl Eq for DeviceInfo {}

impl fmt::Debug for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInfo")
            .field("transport_layer", &self.transport_layer)
            .field("manufacturer", &self.manufacturer)
            .field("model", &self.model)
            .field("serial", &self.serial)
            .field("user_defined_name", &self.user_defined_name)
            .field("current_ip", &self.current_ip)
            .finish()
    }
}
