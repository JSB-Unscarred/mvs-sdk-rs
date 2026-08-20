use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::device::DecodedDevice;
use crate::error::check;
use crate::sys;
use crate::text::{SdkText, sdk_bytes_from_cstr_array};
use crate::{AccessMode, MvsResult, TransportLayer};

/// 枚举设备并复制 SDK 管理的临时记录。
pub(crate) fn enumerate_devices(layers: TransportLayer) -> MvsResult<Vec<DeviceInfo>> {
    let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
    // SAFETY: SDK 写入 raw；Sdk::devices 在复制完成前持有枚举锁。
    check(unsafe { sys::MV_CC_EnumDevices(layers.raw(), &mut raw) })?;

    let device_count = (raw.nDeviceNum as usize).min(raw.pDeviceInfo.len());
    let mut devices = Vec::with_capacity(device_count);
    for ptr in raw.pDeviceInfo.iter().take(device_count) {
        if !ptr.is_null() {
            // SAFETY: non-null 项由本次 EnumDevices 填充，且枚举锁仍在持有。
            devices.push(DeviceInfo {
                raw: Box::new(unsafe { **ptr }),
            });
        }
    }

    Ok(devices)
}

/// 地址稳定的 device record；Clone 会生成独立 snapshot。
pub(crate) struct DeviceInfo {
    raw: Box<sys::MV_CC_DEVICE_INFO>,
}

impl Clone for DeviceInfo {
    fn clone(&self) -> Self {
        Self {
            raw: Box::new(*self.raw),
        }
    }
}

struct DeviceMetadata<'a> {
    manufacturer: &'a [u8],
    model: &'a [u8],
    serial: &'a [u8],
    user_defined_name: &'a [u8],
    device_version: &'a [u8],
    manufacturer_specific_info: &'a [u8],
}

impl DeviceInfo {
    pub(crate) fn raw(&self) -> &sys::MV_CC_DEVICE_INFO {
        &self.raw
    }

    pub(crate) fn decode(&self) -> DecodedDevice {
        let metadata = self.metadata();
        let (current_ip, current_subnet_mask, default_gateway, host_nic_ip) = self.gige_addresses();
        DecodedDevice {
            major_version: self.raw.nMajorVer,
            minor_version: self.raw.nMinorVer,
            mac_address: mac_from_parts(self.raw.nMacAddrHigh, self.raw.nMacAddrLow),
            transport_layer: TransportLayer::from_raw(self.raw.nTLayerType),
            device_type_info: self.raw.nDevTypeInfo,
            manufacturer: sdk_text(metadata.as_ref().map(|metadata| metadata.manufacturer)),
            model: sdk_text(metadata.as_ref().map(|metadata| metadata.model)),
            device_version: sdk_text(metadata.as_ref().map(|metadata| metadata.device_version)),
            manufacturer_specific_info: sdk_text(
                metadata
                    .as_ref()
                    .map(|metadata| metadata.manufacturer_specific_info),
            ),
            serial: sdk_text(metadata.as_ref().map(|metadata| metadata.serial)),
            user_defined_name: sdk_text(
                metadata.as_ref().map(|metadata| metadata.user_defined_name),
            ),
            current_ip,
            current_subnet_mask,
            default_gateway,
            host_nic_ip,
        }
    }

    fn is_gige_transport(&self) -> bool {
        self.raw.nTLayerType == sys::MV_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_VIR_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_GENTL_GIGE_DEVICE
    }

    fn metadata(&self) -> Option<DeviceMetadata<'_>> {
        match self.raw.nTLayerType {
            sys::MV_GIGE_DEVICE | sys::MV_VIR_GIGE_DEVICE | sys::MV_GENTL_GIGE_DEVICE => {
                // SAFETY: transport type 选择 GigE union arm。
                let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &info.chManufacturerSpecificInfo,
                })
            }
            sys::MV_USB_DEVICE | sys::MV_VIR_USB_DEVICE => {
                // SAFETY: transport type 选择 USB3 union arm。
                let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &[],
                })
            }
            sys::MV_CAMERALINK_DEVICE => {
                // SAFETY: transport type 选择 native Camera Link union arm。
                let info = unsafe { &self.raw.SpecialInfo.stCamLInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &[],
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &[],
                })
            }
            sys::MV_GENTL_CAMERALINK_DEVICE => {
                // SAFETY: transport type 选择 GenTL Camera Link union arm。
                let info = unsafe { &self.raw.SpecialInfo.stCMLInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &info.chManufacturerInfo,
                })
            }
            sys::MV_GENTL_CXP_DEVICE => {
                // SAFETY: transport type 选择 CoaXPress union arm。
                let info = unsafe { &self.raw.SpecialInfo.stCXPInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &info.chManufacturerInfo,
                })
            }
            sys::MV_GENTL_XOF_DEVICE => {
                // SAFETY: transport type 选择 XoF union arm。
                let info = unsafe { &self.raw.SpecialInfo.stXoFInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &info.chManufacturerInfo,
                })
            }
            sys::MV_GENTL_VIR_DEVICE => {
                // SAFETY: transport type 选择 GenTL virtual union arm。
                let info = unsafe { &self.raw.SpecialInfo.stVirInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                    device_version: &info.chDeviceVersion,
                    manufacturer_specific_info: &info.chManufacturerInfo,
                })
            }
            _ => None,
        }
    }

    fn gige_addresses(
        &self,
    ) -> (
        Option<Ipv4Addr>,
        Option<Ipv4Addr>,
        Option<Ipv4Addr>,
        Option<Ipv4Addr>,
    ) {
        if !self.is_gige_transport() {
            return (None, None, None, None);
        }
        // SAFETY: is_gige_transport 只接受 GigE union arm 对应的 transport type。
        let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
        (
            Some(ipv4_from_sdk(info.nCurrentIp)),
            Some(ipv4_from_sdk(info.nCurrentSubNetMask)),
            Some(ipv4_from_sdk(info.nDefultGateWay)),
            Some(ipv4_from_sdk(info.nNetExport)),
        )
    }

    pub(crate) fn is_accessible(&self, mode: AccessMode) -> bool {
        // C API 使用 mutable pointer；私有副本避免把共享 Rust 数据暴露为 *mut。
        let mut raw = *self.raw;
        // SAFETY: raw 是枚举所得 record 的完整本地副本。
        unsafe { sys::MV_CC_IsDeviceAccessible(&mut raw, mode.raw()) != 0 }
    }

    pub(crate) fn as_raw(&self) -> *const c_void {
        std::ptr::from_ref(self.raw.as_ref()).cast()
    }
}

fn sdk_text(bytes: Option<&[u8]>) -> SdkText {
    SdkText::from_sdk_bytes(bytes.map(sdk_bytes_from_cstr_array).unwrap_or_default())
}

fn mac_from_parts(high: u32, low: u32) -> [u8; 8] {
    let mut mac = [0_u8; 8];
    mac[..4].copy_from_slice(&high.to_be_bytes());
    mac[4..].copy_from_slice(&low.to_be_bytes());
    mac
}

fn ipv4_from_sdk(value: u32) -> Ipv4Addr {
    Ipv4Addr::from(value.to_be_bytes())
}
