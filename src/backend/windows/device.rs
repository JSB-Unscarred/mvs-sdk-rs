use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::error::check;
use crate::sys;
use crate::{AccessMode, MvsResult, TransportLayer};

/// 保存枚举结果的 Rust-owned snapshot。
pub(crate) struct DeviceList {
    devices: Vec<Arc<sys::MV_CC_DEVICE_INFO>>,
}

impl DeviceList {
    pub(crate) fn enumerate(layers: TransportLayer) -> MvsResult<Self> {
        let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
        // SAFETY: SDK 写入 raw；Sdk::enumerate_devices 在复制完成前持有枚举锁。
        check(unsafe { sys::MV_CC_EnumDevices(layers.raw(), &mut raw) })?;

        let device_count = (raw.nDeviceNum as usize).min(raw.pDeviceInfo.len());
        let mut devices = Vec::with_capacity(device_count);
        for ptr in raw.pDeviceInfo.iter().take(device_count) {
            if !ptr.is_null() {
                // SAFETY: non-null 项由本次 EnumDevices 填充，且枚举锁仍在持有。
                devices.push(Arc::new(unsafe { **ptr }));
            }
        }

        Ok(Self { devices })
    }

    pub(crate) fn len(&self) -> usize {
        self.devices.len()
    }

    pub(crate) fn get(&self, index: usize) -> Option<DeviceInfo> {
        self.devices.get(index).map(|raw| DeviceInfo {
            raw: Arc::clone(raw),
        })
    }
}

/// 地址稳定的单个 device record，由列表与 clone 共享所有权。
#[derive(Clone)]
pub(crate) struct DeviceInfo {
    raw: Arc<sys::MV_CC_DEVICE_INFO>,
}

struct DeviceMetadata<'a> {
    manufacturer: &'a [u8],
    model: &'a [u8],
    serial: &'a [u8],
    user_defined_name: &'a [u8],
}

impl DeviceInfo {
    pub(crate) fn raw(&self) -> &sys::MV_CC_DEVICE_INFO {
        self.raw.as_ref()
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
        Arc::as_ptr(&self.raw).cast()
    }
}

fn cstr_array_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    use super::{DeviceInfo, DeviceList};
    use crate::sys;

    fn c_string<const N: usize>(value: &str) -> [u8; N] {
        let mut bytes = [0; N];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        bytes
    }

    fn assert_metadata(info: &DeviceInfo, expected: (&str, &str, &str, &str)) {
        assert_eq!(
            (
                info.manufacturer(),
                info.model(),
                info.serial(),
                info.user_defined_name(),
            ),
            (
                expected.0.into(),
                expected.1.into(),
                expected.2.into(),
                expected.3.into(),
            )
        );
    }

    // 验证 snapshot 地址由 Arc 保持稳定，并按 transport 选择各 union arm。
    #[test]
    fn device_snapshots_are_stable_and_decode_transport_metadata() {
        let mut gige_raw = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_GIGE_DEVICE,
            ..Default::default()
        };
        gige_raw.SpecialInfo.stGigEInfo = sys::MV_GIGE_DEVICE_INFO {
            chManufacturerName: c_string("Hikrobot"),
            chModelName: c_string("GigE-42"),
            chSerialNumber: c_string("GE-0001"),
            chUserDefinedName: c_string("line-a"),
            nCurrentIp: u32::from_be_bytes([192, 168, 1, 64]),
            nNetExport: u32::from_be_bytes([192, 168, 1, 10]),
            ..Default::default()
        };
        let list = DeviceList {
            devices: vec![Arc::new(gige_raw)],
        };
        let gige = list.get(0).unwrap();
        let raw_address = gige.as_raw();
        let clone = gige.clone();
        drop(list);
        assert_eq!(clone.as_raw(), raw_address);
        assert_metadata(&gige, ("Hikrobot", "GigE-42", "GE-0001", "line-a"));
        assert_eq!(gige.ip(), Some(Ipv4Addr::new(192, 168, 1, 64)));
        assert_eq!(gige.host_nic_ip(), Some(Ipv4Addr::new(192, 168, 1, 10)));

        let mut usb_raw = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_USB_DEVICE,
            ..Default::default()
        };
        usb_raw.SpecialInfo.stUsb3VInfo = sys::MV_USB3_DEVICE_INFO {
            chManufacturerName: c_string("USB Vendor"),
            chModelName: c_string("USB Model"),
            chSerialNumber: c_string("USB Serial"),
            chUserDefinedName: c_string("USB User"),
            ..Default::default()
        };
        let usb = DeviceInfo {
            raw: Arc::new(usb_raw),
        };
        assert_metadata(&usb, ("USB Vendor", "USB Model", "USB Serial", "USB User"));
        assert_eq!(usb.ip(), None);

        let mut camera_link_raw = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_CAMERALINK_DEVICE,
            ..Default::default()
        };
        camera_link_raw.SpecialInfo.stCamLInfo = sys::MV_CamL_DEV_INFO {
            chManufacturerName: c_string("CL Vendor"),
            chModelName: c_string("CL Model"),
            chSerialNumber: c_string("CL Serial"),
            ..Default::default()
        };
        assert_metadata(
            &DeviceInfo {
                raw: Arc::new(camera_link_raw),
            },
            ("CL Vendor", "CL Model", "CL Serial", ""),
        );

        macro_rules! assert_gentl_arm {
            ($layer:expr, $field:ident, $info:ident) => {{
                let mut raw = sys::MV_CC_DEVICE_INFO {
                    nTLayerType: $layer,
                    ..Default::default()
                };
                raw.SpecialInfo.$field = sys::$info {
                    chVendorName: c_string("GenTL Vendor"),
                    chModelName: c_string("GenTL Model"),
                    chSerialNumber: c_string("GenTL Serial"),
                    chUserDefinedName: c_string("GenTL User"),
                    ..Default::default()
                };
                assert_metadata(
                    &DeviceInfo { raw: Arc::new(raw) },
                    ("GenTL Vendor", "GenTL Model", "GenTL Serial", "GenTL User"),
                );
            }};
        }

        assert_gentl_arm!(
            sys::MV_GENTL_CAMERALINK_DEVICE,
            stCMLInfo,
            MV_CML_DEVICE_INFO
        );
        assert_gentl_arm!(sys::MV_GENTL_CXP_DEVICE, stCXPInfo, MV_CXP_DEVICE_INFO);
        assert_gentl_arm!(sys::MV_GENTL_XOF_DEVICE, stXoFInfo, MV_XOF_DEVICE_INFO);
        assert_gentl_arm!(
            sys::MV_GENTL_VIR_DEVICE,
            stVirInfo,
            MV_GENTL_VIR_DEVICE_INFO
        );
    }
}
