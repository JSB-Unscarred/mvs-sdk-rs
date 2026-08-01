use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::sync::Mutex;

use crate::error::check;
use crate::sys;
use crate::{AccessMode, MvsResult, TransportLayer};

// MV_CC_EnumDevices returns pointers into SDK-owned storage. The vendor
// documents that another enumeration may release and replace that storage, so
// all safe-wrapper enumerations must be serialized until every record is
// copied into Rust-owned memory.
static DEVICE_ENUMERATION_LOCK: Mutex<()> = Mutex::new(());

fn with_raw_device_list<T>(
    layers: TransportLayer,
    enumerate: impl FnOnce(u32, &mut sys::MV_CC_DEVICE_INFO_LIST) -> i32,
    snapshot: impl FnOnce(&sys::MV_CC_DEVICE_INFO_LIST) -> T,
) -> MvsResult<T> {
    let _guard = DEVICE_ENUMERATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
    check(enumerate(layers.raw(), &mut raw))?;
    Ok(snapshot(&raw))
}

pub(crate) struct DeviceList {
    devices: Vec<sys::MV_CC_DEVICE_INFO>,
}

impl DeviceList {
    pub(crate) fn enumerate(layers: TransportLayer) -> MvsResult<Self> {
        let devices = with_raw_device_list(
            layers,
            |raw_layers, raw| {
                // SAFETY: the SDK fills `raw`; the process-wide lock remains
                // held while the snapshot closure copies its device records.
                unsafe { sys::MV_CC_EnumDevices(raw_layers, raw) }
            },
            |raw| {
                let device_count = (raw.nDeviceNum as usize).min(raw.pDeviceInfo.len());
                let mut devices = Vec::with_capacity(device_count);
                for ptr in raw.pDeviceInfo.iter().take(device_count) {
                    if !ptr.is_null() {
                        // SAFETY: every non-null pointer was populated by
                        // EnumDevices, and no other safe enumeration can
                        // replace the SDK-owned storage while the lock is held.
                        devices.push(unsafe { **ptr });
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

    pub(crate) fn get(&self, index: usize) -> Option<DeviceInfo<'_>> {
        self.devices.get(index).map(|raw| DeviceInfo { raw })
    }
}

#[derive(Copy, Clone)]
pub(crate) struct DeviceInfo<'a> {
    raw: &'a sys::MV_CC_DEVICE_INFO,
}

impl DeviceInfo<'_> {
    pub(crate) fn raw(&self) -> &sys::MV_CC_DEVICE_INFO {
        self.raw
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

    pub(crate) fn manufacturer(&self) -> String {
        if self.is_gige() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stGigEInfo.chManufacturerName })
        } else if self.is_usb() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stUsb3VInfo.chManufacturerName })
        } else {
            String::new()
        }
    }

    pub(crate) fn model(&self) -> String {
        if self.is_gige() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stGigEInfo.chModelName })
        } else if self.is_usb() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stUsb3VInfo.chModelName })
        } else {
            String::new()
        }
    }

    pub(crate) fn serial(&self) -> String {
        if self.is_gige() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stGigEInfo.chSerialNumber })
        } else if self.is_usb() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stUsb3VInfo.chSerialNumber })
        } else {
            String::new()
        }
    }

    pub(crate) fn user_defined_name(&self) -> String {
        if self.is_gige() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stGigEInfo.chUserDefinedName })
        } else if self.is_usb() {
            // SAFETY: the union arm is selected by nTLayerType.
            cstr_array_to_string(unsafe { &self.raw.SpecialInfo.stUsb3VInfo.chUserDefinedName })
        } else {
            String::new()
        }
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
        self.raw as *const _ as *const c_void
    }
}

fn cstr_array_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::TryLockError;

    use super::{DEVICE_ENUMERATION_LOCK, with_raw_device_list};
    use crate::{TransportLayer, sys};

    #[test]
    fn device_records_are_snapshotted_while_enumeration_lock_is_held() {
        let mut device = sys::MV_CC_DEVICE_INFO {
            nTLayerType: sys::MV_USB_DEVICE,
            ..Default::default()
        };

        let devices = with_raw_device_list(
            TransportLayer::USB,
            |layers, raw| {
                assert_eq!(layers, sys::MV_USB_DEVICE);
                raw.nDeviceNum = 1;
                raw.pDeviceInfo[0] = &mut device;
                sys::MV_OK as i32
            },
            |raw| {
                assert!(matches!(
                    DEVICE_ENUMERATION_LOCK.try_lock(),
                    Err(TryLockError::WouldBlock)
                ));
                vec![unsafe { *raw.pDeviceInfo[0] }]
            },
        )
        .unwrap();

        assert_eq!(devices[0].nTLayerType, sys::MV_USB_DEVICE);
    }
}
