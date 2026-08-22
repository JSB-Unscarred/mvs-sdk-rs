//! Rust-owned 设备 metadata snapshot。

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::backend;
use crate::text::SdkText;
use crate::{AccessMode, TransportLayer};

/// 从 SDK 枚举记录解出的设备字段。
///
/// 公开字段是 C 结构体中跨 transport 的常用子集，字符串保留原始字节。
/// 字段清单只在此处定义，[`DeviceInfo`] 直接内嵌本值。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeviceProperties {
    /// `MV_CC_DEVICE_INFO::nMajorVer`。
    pub major_version: u16,
    /// `MV_CC_DEVICE_INFO::nMinorVer`。
    pub minor_version: u16,
    /// `nMacAddrHigh` 与 `nMacAddrLow` 按大端拼成 8 字节。
    pub mac_address: [u8; 8],
    /// `MV_CC_DEVICE_INFO::nTLayerType`，SDK 为设备写入单一 device type 值。
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
}

/// 从 SDK 枚举结果深拷贝得到的设备 snapshot。
///
/// 本值不持有 SDK session lease；打开与可访问性查询由 [`crate::Sdk`] 提供。
/// 解码字段见 [`DeviceProperties`]，内部另存同一记录的 C 快照供 CreateHandle 使用。
#[derive(Clone)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// 解码后的公开字段。
    pub properties: DeviceProperties,
    inner: backend::DeviceInfo,
}

impl DeviceInfo {
    pub(crate) fn from_backend(inner: backend::DeviceInfo) -> Self {
        Self {
            properties: inner.decode(),
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
        self.properties.transport_layer.is_gige()
    }

    /// 返回设备是否使用 USB transport。
    #[must_use]
    pub fn is_usb(&self) -> bool {
        self.properties.transport_layer.is_usb()
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
    /// 相等性只由解码字段决定；`inner` 是同一条记录的 C 快照。
    fn eq(&self, other: &Self) -> bool {
        self.properties == other.properties
    }
}

impl Eq for DeviceInfo {}

impl fmt::Debug for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInfo")
            .field("transport_layer", &self.properties.transport_layer)
            .field("manufacturer", &self.properties.manufacturer)
            .field("model", &self.properties.model)
            .field("serial", &self.properties.serial)
            .field("user_defined_name", &self.properties.user_defined_name)
            .field("current_ip", &self.properties.current_ip)
            .finish()
    }
}
