use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::error::check;
use crate::sys;
use crate::{AccessMode, MvsResult, TransportLayer};

fn with_raw_device_list<T>(
    layers: TransportLayer,
    enumerate: impl FnOnce(u32, &mut sys::MV_CC_DEVICE_INFO_LIST) -> i32,
    snapshot: impl FnOnce(&sys::MV_CC_DEVICE_INFO_LIST) -> T,
) -> MvsResult<T> {
    let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
    check(enumerate(layers.raw(), &mut raw))?;
    Ok(snapshot(&raw))
}

pub(crate) struct DeviceList {
    devices: Vec<Arc<sys::MV_CC_DEVICE_INFO>>,
}

impl DeviceList {
    pub(crate) fn enumerate(layers: TransportLayer) -> MvsResult<Self> {
        let devices = with_raw_device_list(
            layers,
            |raw_layers, raw| {
                // SAFETY: the SDK fills `raw`; Sdk::enumerate_devices holds
                // the singleton's enumeration lock through this snapshot.
                unsafe { sys::MV_CC_EnumDevices(raw_layers, raw) }
            },
            |raw| {
                let device_count = (raw.nDeviceNum as usize).min(raw.pDeviceInfo.len());
                let mut devices = Vec::with_capacity(device_count);
                for ptr in raw.pDeviceInfo.iter().take(device_count) {
                    if !ptr.is_null() {
                        // SAFETY: every non-null pointer was populated by
                        // EnumDevices, and the outer singleton lock prevents a
                        // second safe enumeration until this copy completes.
                        devices.push(Arc::new(unsafe { **ptr }));
                    }
                }

                devices
            },
        )?;

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
                // SAFETY: these transport values select the GigE union arm.
                let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                })
            }
            sys::MV_USB_DEVICE | sys::MV_VIR_USB_DEVICE => {
                // SAFETY: these transport values select the USB3 union arm.
                let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                })
            }
            sys::MV_CAMERALINK_DEVICE => {
                // SAFETY: this transport value selects the native Camera Link
                // union arm, which does not define a user-defined name.
                let info = unsafe { &self.raw.SpecialInfo.stCamLInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chManufacturerName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &[],
                })
            }
            sys::MV_GENTL_CAMERALINK_DEVICE => {
                // SAFETY: this transport value selects the GenTL Camera Link
                // union arm.
                let info = unsafe { &self.raw.SpecialInfo.stCMLInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                })
            }
            sys::MV_GENTL_CXP_DEVICE => {
                // SAFETY: this transport value selects the CoaXPress union arm.
                let info = unsafe { &self.raw.SpecialInfo.stCXPInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                })
            }
            sys::MV_GENTL_XOF_DEVICE => {
                // SAFETY: this transport value selects the XoF union arm.
                let info = unsafe { &self.raw.SpecialInfo.stXoFInfo };
                Some(DeviceMetadata {
                    manufacturer: &info.chVendorName,
                    model: &info.chModelName,
                    serial: &info.chSerialNumber,
                    user_defined_name: &info.chUserDefinedName,
                })
            }
            sys::MV_GENTL_VIR_DEVICE => {
                // SAFETY: this transport value selects the GenTL virtual union
                // arm.
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
            // SAFETY: the union arm is selected by nTLayerType.
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nCurrentIp.to_be_bytes()))
        } else {
            None
        }
    }

    pub(crate) fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        if self.is_gige() {
            // SAFETY: the union arm is selected by nTLayerType.
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nNetExport.to_be_bytes()))
        } else {
            None
        }
    }

    pub(crate) fn is_accessible(&self, mode: AccessMode) -> bool {
        // The C API takes a mutable pointer even though this is a query. Give
        // it a private copy so concurrent queries never expose shared Rust
        // data through `*mut`.
        let mut raw = *self.raw;
        // SAFETY: `raw` is a valid local copy of the enumerated device record.
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
        assert!(value.len() < N);
        let mut bytes = [0; N];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        bytes
    }

    fn assert_metadata(
        info: &DeviceInfo,
        manufacturer: &str,
        model: &str,
        serial: &str,
        user_defined_name: &str,
    ) {
        assert_eq!(info.manufacturer(), manufacturer);
        assert_eq!(info.model(), model);
        assert_eq!(info.serial(), serial);
        assert_eq!(info.user_defined_name(), user_defined_name);
    }

    // 验证 DeviceInfo clone 共享稳定地址，并延长 Rust-owned record 生命周期。
    #[test]
    fn device_info_owns_an_address_stable_record() {
        let list = DeviceList {
            devices: vec![Arc::new(sys::MV_CC_DEVICE_INFO {
                nTLayerType: sys::MV_USB_DEVICE,
                ..Default::default()
            })],
        };

        let info = list.get(0).unwrap();
        let raw = info.as_raw().cast::<sys::MV_CC_DEVICE_INFO>();
        let cloned: DeviceInfo = info.clone();
        drop(list);

        assert_eq!(info.as_raw(), raw.cast());
        assert_eq!(cloned.as_raw(), raw.cast());
        // SAFETY: `info` and `cloned` keep the Arc allocation alive.
        assert_eq!(unsafe { (*raw).nTLayerType }, sys::MV_USB_DEVICE);
        assert_eq!(cloned.raw().nTLayerType, sys::MV_USB_DEVICE);
    }

    // 验证 GigE/USB 及其 alias 选择正确 union arm 与网络字段。
    #[test]
    fn device_metadata_decodes_gige_and_usb_union_records() {
        let mut gige_record = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_GIGE_DEVICE,
            ..Default::default()
        };
        gige_record.SpecialInfo.stGigEInfo = sys::MV_GIGE_DEVICE_INFO {
            chManufacturerName: c_string("Hikrobot"),
            chModelName: c_string("GigE-42"),
            chSerialNumber: c_string("GE-0001"),
            chUserDefinedName: c_string("line-a"),
            nCurrentIp: u32::from_be_bytes([192, 168, 1, 64]),
            nNetExport: u32::from_be_bytes([192, 168, 1, 10]),
            ..Default::default()
        };
        let gige = DeviceInfo {
            raw: Arc::new(gige_record),
        };

        assert_eq!(
            (
                gige.manufacturer(),
                gige.model(),
                gige.serial(),
                gige.user_defined_name(),
                gige.ip(),
                gige.host_nic_ip(),
            ),
            (
                "Hikrobot".into(),
                "GigE-42".into(),
                "GE-0001".into(),
                "line-a".into(),
                Some(Ipv4Addr::new(192, 168, 1, 64)),
                Some(Ipv4Addr::new(192, 168, 1, 10)),
            )
        );
        for layer in [sys::MV_VIR_GIGE_DEVICE, sys::MV_GENTL_GIGE_DEVICE] {
            let mut alias_record = gige_record;
            alias_record.nTLayerType = layer;
            let alias = DeviceInfo {
                raw: Arc::new(alias_record),
            };
            assert_metadata(&alias, "Hikrobot", "GigE-42", "GE-0001", "line-a");
        }

        let mut usb_record = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_USB_DEVICE,
            ..Default::default()
        };
        usb_record.SpecialInfo.stUsb3VInfo = sys::MV_USB3_DEVICE_INFO {
            chManufacturerName: c_string("Vision USB"),
            chModelName: c_string("USB-7"),
            chSerialNumber: c_string("USB-0002"),
            chUserDefinedName: c_string("bench"),
            ..Default::default()
        };
        let usb = DeviceInfo {
            raw: Arc::new(usb_record),
        };

        assert_eq!(
            (
                usb.manufacturer(),
                usb.model(),
                usb.serial(),
                usb.user_defined_name(),
                usb.ip(),
                usb.host_nic_ip(),
            ),
            (
                "Vision USB".into(),
                "USB-7".into(),
                "USB-0002".into(),
                "bench".into(),
                None,
                None,
            )
        );
        let mut virtual_usb_record = usb_record;
        virtual_usb_record.nTLayerType = sys::MV_VIR_USB_DEVICE;
        let virtual_usb = DeviceInfo {
            raw: Arc::new(virtual_usb_record),
        };
        assert_metadata(&virtual_usb, "Vision USB", "USB-7", "USB-0002", "bench");
    }

    // 验证 Camera Link/GenTL transport 选择各自 union arm，未知字段返回空值。
    #[test]
    fn device_metadata_decodes_camera_link_and_gentl_union_records() {
        let mut camera_link_record = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_CAMERALINK_DEVICE,
            ..Default::default()
        };
        camera_link_record.SpecialInfo.stCamLInfo = sys::MV_CamL_DEV_INFO {
            chManufacturerName: c_string("Camera Link Vendor"),
            chModelName: c_string("CL-1"),
            chSerialNumber: c_string("CL-0001"),
            ..Default::default()
        };
        let camera_link = DeviceInfo {
            raw: Arc::new(camera_link_record),
        };
        assert_metadata(&camera_link, "Camera Link Vendor", "CL-1", "CL-0001", "");

        macro_rules! assert_gentl_metadata {
            ($layer:expr, $field:ident, $info:ident) => {{
                let mut record = sys::MV_CC_DEVICE_INFO {
                    nTLayerType: $layer,
                    ..Default::default()
                };
                record.SpecialInfo.$field = sys::$info {
                    chVendorName: c_string("GenTL Vendor"),
                    chModelName: c_string("GenTL Model"),
                    chSerialNumber: c_string("GenTL Serial"),
                    chUserDefinedName: c_string("GenTL User"),
                    ..Default::default()
                };
                let info = DeviceInfo {
                    raw: Arc::new(record),
                };
                assert_metadata(
                    &info,
                    "GenTL Vendor",
                    "GenTL Model",
                    "GenTL Serial",
                    "GenTL User",
                );
            }};
        }

        assert_gentl_metadata!(
            sys::MV_GENTL_CAMERALINK_DEVICE,
            stCMLInfo,
            MV_CML_DEVICE_INFO
        );
        assert_gentl_metadata!(sys::MV_GENTL_CXP_DEVICE, stCXPInfo, MV_CXP_DEVICE_INFO);
        assert_gentl_metadata!(sys::MV_GENTL_XOF_DEVICE, stXoFInfo, MV_XOF_DEVICE_INFO);
        assert_gentl_metadata!(
            sys::MV_GENTL_VIR_DEVICE,
            stVirInfo,
            MV_GENTL_VIR_DEVICE_INFO
        );

        let ieee_1394 = DeviceInfo {
            raw: Arc::new(sys::MV_CC_DEVICE_INFO {
                nTLayerType: sys::MV_1394_DEVICE,
                ..Default::default()
            }),
        };
        assert_metadata(&ieee_1394, "", "", "", "");
    }
}
