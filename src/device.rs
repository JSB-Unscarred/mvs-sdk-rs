//! Rust-owned 设备 metadata snapshot。

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::AccessMode;
use crate::backend;

/// 从 SDK 枚举结果深拷贝得到的设备 snapshot。
///
/// 本值不持有 SDK session lease；需要 native session 的可访问性查询与打开操作由
/// [`crate::Sdk`] 提供。
#[derive(Clone)]
pub struct DeviceInfo {
    inner: backend::DeviceInfo,
}

impl DeviceInfo {
    pub(crate) fn from_backend(inner: backend::DeviceInfo) -> Self {
        Self { inner }
    }

    pub(crate) fn clone_backend(&self) -> backend::DeviceInfo {
        self.inner.clone()
    }

    pub(crate) fn is_accessible(&self, mode: AccessMode) -> bool {
        self.inner.is_accessible(mode)
    }

    /// 返回 SDK 报告的 transport layer。
    pub fn transport_layer(&self) -> crate::TransportLayer {
        self.inner.transport_layer()
    }

    /// 返回设备是否使用 GigE transport。
    pub fn is_gige(&self) -> bool {
        self.inner.is_gige()
    }

    /// 返回设备是否使用 USB transport。
    pub fn is_usb(&self) -> bool {
        self.inner.is_usb()
    }

    /// 返回 manufacturer name；无对应字段时为空字符串。
    pub fn manufacturer(&self) -> String {
        self.inner.manufacturer()
    }

    /// 返回 model name；无对应字段时为空字符串。
    pub fn model(&self) -> String {
        self.inner.model()
    }

    /// 返回 serial number；无对应字段时为空字符串。
    pub fn serial(&self) -> String {
        self.inner.serial()
    }

    /// 返回 user-defined name；无对应字段时为空字符串。
    pub fn user_defined_name(&self) -> String {
        self.inner.user_defined_name()
    }

    /// 返回 GigE 设备当前 IP，其它 transport 返回 `None`。
    pub fn ip(&self) -> Option<Ipv4Addr> {
        self.inner.ip()
    }

    /// 返回 GigE 设备使用的 host NIC IP，其它 transport 返回 `None`。
    pub fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        self.inner.host_nic_ip()
    }

    /// 借出当前 owned snapshot 的 opaque pointer。
    ///
    /// pointer 只在本值存活期间有效；调用 raw SDK 仍属于 `unsafe` 操作。
    pub fn as_raw(&self) -> *const c_void {
        self.inner.as_raw()
    }
}

impl fmt::Debug for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceInfo")
            .field("transport", &self.transport_layer())
            .field("manufacturer", &self.manufacturer())
            .field("model", &self.model())
            .field("serial", &self.serial())
            .field("user_defined_name", &self.user_defined_name())
            .field("ip", &self.ip())
            .finish()
    }
}
