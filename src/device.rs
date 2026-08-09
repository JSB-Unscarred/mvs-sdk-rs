//! 设备枚举结果与设备 metadata。

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::backend;
use crate::camera::Camera;
use crate::library::Sdk;
use crate::{AccessMode, MvsResult, TransportLayer};

/// Rust-owned 设备 snapshot 列表。
///
/// 列表借用 [`Sdk`]，保证其条目传回 native API 时 SDK 仍处于活动期。
pub struct DeviceList<'sdk> {
    devices: Vec<DeviceInfo<'sdk>>,
}

impl<'sdk> DeviceList<'sdk> {
    pub(crate) fn enumerate(sdk: &'sdk Sdk, layers: TransportLayer) -> MvsResult<Self> {
        let devices = backend::DeviceList::enumerate(layers)?
            .into_devices()
            .into_iter()
            .map(|inner| DeviceInfo { inner, sdk })
            .collect();
        Ok(Self { devices })
    }

    /// 返回设备数量。
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 返回列表是否为空。
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 按枚举顺序借用设备 snapshot。
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &DeviceInfo<'sdk>> {
        self.devices.iter()
    }

    /// 借用指定位置的设备 snapshot。
    pub fn get(&self, index: usize) -> Option<&DeviceInfo<'sdk>> {
        self.devices.get(index)
    }
}

impl fmt::Debug for DeviceList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceList")
            .field("count", &self.len())
            .finish()
    }
}

/// 从 SDK 枚举结果深拷贝得到的单个设备 snapshot。
#[derive(Clone)]
pub struct DeviceInfo<'sdk> {
    inner: backend::DeviceInfo,
    sdk: &'sdk Sdk,
}

impl<'sdk> DeviceInfo<'sdk> {
    /// 返回 SDK 报告的 transport layer。
    pub fn transport_layer(&self) -> TransportLayer {
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

    /// 查询设备是否可按指定权限打开。
    pub fn is_accessible(&self, mode: AccessMode) -> bool {
        self.inner.is_accessible(mode)
    }

    /// 按官方 `OpenDevice` 的 access mode 与 switchover key 打开设备。
    ///
    /// key 仅对 native GigE 设备有意义；其它 transport 由 SDK 忽略。
    pub fn open(&self, mode: AccessMode, switchover_key: u16) -> MvsResult<Camera<'sdk>> {
        Camera::open(self.inner.clone(), self.sdk, mode, switchover_key)
    }

    /// 借出当前 owned snapshot 的 opaque pointer。
    ///
    /// pointer 只在本值存活期间有效；调用 raw SDK 仍属于 `unsafe` 操作。
    pub fn as_raw(&self) -> *const c_void {
        self.inner.as_raw()
    }
}

impl fmt::Debug for DeviceInfo<'_> {
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
