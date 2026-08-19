use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::error::check;
use crate::sys;
use crate::{AccessMode, MvsResult, TransportLayer};

/// 枚举设备并复制 SDK 管理的临时记录。
pub(crate) fn enumerate_devices(layers: TransportLayer) -> MvsResult<Vec<DeviceInfo>> {
    let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
    // SAFETY: SDK 写入 raw；Sdk::enumerate_devices 在复制完成前持有枚举锁。
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
}

impl DeviceInfo {
    pub(crate) fn raw(&self) -> &sys::MV_CC_DEVICE_INFO {
        &self.raw
    }

    pub(crate) fn transport_layer(&self) -> TransportLayer {
        TransportLayer::from_raw(self.raw.nTLayerType)
    }

    pub(crate) fn is_gige(&self) -> bool {
        self.raw.nTLayerType == sys::MV_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_VIR_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_GENTL_GIGE_DEVICE
    }

    pub(crate) fn is_usb(&self) -> bool {
        self.raw.nTLayerType == sys::MV_USB_DEVICE || self.raw.nTLayerType == sys::MV_VIR_USB_DEVICE
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
                })
            }
            _ => None,
        }
    }

    pub(crate) fn manufacturer(&self) -> String {
        self.metadata()
            .map(|metadata| cstr_array_to_string(metadata.manufacturer))
            .unwrap_or_default()
    }

    pub(crate) fn model(&self) -> String {
        self.metadata()
            .map(|metadata| cstr_array_to_string(metadata.model))
            .unwrap_or_default()
    }

    pub(crate) fn serial(&self) -> String {
        self.metadata()
            .map(|metadata| cstr_array_to_string(metadata.serial))
            .unwrap_or_default()
    }

    pub(crate) fn user_defined_name(&self) -> String {
        self.metadata()
            .map(|metadata| cstr_array_to_string(metadata.user_defined_name))
            .unwrap_or_default()
    }

    pub(crate) fn ip(&self) -> Option<Ipv4Addr> {
        if self.is_gige() {
            // SAFETY: is_gige 只接受 GigE union arm 对应的 transport type。
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nCurrentIp.to_be_bytes()))
        } else {
            None
        }
    }

    pub(crate) fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        if self.is_gige() {
            // SAFETY: is_gige 只接受 GigE union arm 对应的 transport type。
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nNetExport.to_be_bytes()))
        } else {
            None
        }
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

fn cstr_array_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
