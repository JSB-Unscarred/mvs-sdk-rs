//! Device enumeration and per-device metadata.

use std::fmt;
use std::net::Ipv4Addr;
use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use crate::MvsResult;
use crate::camera::{AccessMode, Camera};
use crate::error::check;
use crate::library::Sdk;
use crate::sys;

// ---------------------------------------------------------------------------
// TransportLayer (bitflag over u32)
// ---------------------------------------------------------------------------

/// Bit set of transport-layer protocols to enumerate. Combine with `|`.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct TransportLayer(u32);

impl TransportLayer {
    pub const UNKNOWN: Self = Self(sys::MV_UNKNOW_DEVICE);
    pub const GIGE: Self = Self(sys::MV_GIGE_DEVICE);
    pub const USB: Self = Self(sys::MV_USB_DEVICE);
    pub const CAMERALINK: Self = Self(sys::MV_CAMERALINK_DEVICE);
    pub const VIR_GIGE: Self = Self(sys::MV_VIR_GIGE_DEVICE);
    pub const VIR_USB: Self = Self(sys::MV_VIR_USB_DEVICE);
    pub const GENTL_GIGE: Self = Self(sys::MV_GENTL_GIGE_DEVICE);
    pub const GENTL_CAMERALINK: Self = Self(sys::MV_GENTL_CAMERALINK_DEVICE);
    pub const GENTL_CXP: Self = Self(sys::MV_GENTL_CXP_DEVICE);
    pub const GENTL_XOF: Self = Self(sys::MV_GENTL_XOF_DEVICE);
    pub const GENTL_VIR: Self = Self(sys::MV_GENTL_VIR_DEVICE);

    /// Enumerate every type the SDK knows about.
    pub const ALL: Self = Self(0xFFFF_FFFF);

    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for TransportLayer {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for TransportLayer {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for TransportLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransportLayer(0x{:08X})", self.0)
    }
}

// ---------------------------------------------------------------------------
// DeviceList
// ---------------------------------------------------------------------------

/// Owned list of enumerated devices. Iterate via [`DeviceList::iter`].
///
/// The underlying storage is `sys::MV_CC_DEVICE_INFO_LIST` (an array of
/// pointers into SDK-owned memory). The SDK guarantees these pointers remain
/// valid between `EnumDevices` calls, so `DeviceList` retains an [`Arc`] to
/// the [`Sdk`] to ensure the SDK stays initialized.
pub struct DeviceList {
    raw: sys::MV_CC_DEVICE_INFO_LIST,
    library: Arc<Sdk>,
}

impl DeviceList {
    pub(crate) fn enumerate(library: &Arc<Sdk>, layers: TransportLayer) -> MvsResult<Self> {
        let mut raw = sys::MV_CC_DEVICE_INFO_LIST::default();
        // SAFETY: SDK fills the list in-place; `raw` stays on the stack until
        // moved into the returned value.
        let code = unsafe { sys::MV_CC_EnumDevices(layers.raw(), &mut raw) };
        check(code)?;
        Ok(Self {
            raw,
            library: Arc::clone(library),
        })
    }

    /// Number of devices found.
    pub fn len(&self) -> usize {
        self.raw.nDeviceNum as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> DeviceIter<'_> {
        DeviceIter {
            list: self,
            index: 0,
        }
    }

    pub fn get(&self, index: usize) -> Option<DeviceInfo<'_>> {
        if index >= self.len() {
            return None;
        }
        let ptr = self.raw.pDeviceInfo[index];
        if ptr.is_null() {
            return None;
        }
        // SAFETY: SDK guarantees pointer validity for the list's lifetime.
        Some(DeviceInfo {
            raw: unsafe { &*ptr },
            library: &self.library,
        })
    }
}

impl fmt::Debug for DeviceList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceList")
            .field("count", &self.len())
            .finish()
    }
}

// DeviceList owns SDK-internal pointers but they are not Send-safe by default.
// However the SDK documents that enumerated lists can be read from any thread.
// SAFETY: Sdk initialization is ref-counted; pointers stay valid.
unsafe impl Send for DeviceList {}
unsafe impl Sync for DeviceList {}

// ---------------------------------------------------------------------------
// DeviceIter
// ---------------------------------------------------------------------------

pub struct DeviceIter<'a> {
    list: &'a DeviceList,
    index: usize,
}

impl<'a> Iterator for DeviceIter<'a> {
    type Item = DeviceInfo<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let info = self.list.get(self.index)?;
        self.index += 1;
        Some(info)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.list.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for DeviceIter<'_> {}

// ---------------------------------------------------------------------------
// DeviceInfo
// ---------------------------------------------------------------------------

/// Borrowed view of one entry in a [`DeviceList`].
#[derive(Copy, Clone)]
pub struct DeviceInfo<'a> {
    raw: &'a sys::MV_CC_DEVICE_INFO,
    library: &'a Arc<Sdk>,
}

impl<'a> DeviceInfo<'a> {
    /// Transport-layer protocol used by this device.
    pub fn transport_layer(&self) -> TransportLayer {
        TransportLayer::from_raw(self.raw.nTLayerType)
    }

    pub fn is_gige(&self) -> bool {
        self.raw.nTLayerType == sys::MV_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_VIR_GIGE_DEVICE
            || self.raw.nTLayerType == sys::MV_GENTL_GIGE_DEVICE
    }

    pub fn is_usb(&self) -> bool {
        self.raw.nTLayerType == sys::MV_USB_DEVICE || self.raw.nTLayerType == sys::MV_VIR_USB_DEVICE
    }

    /// Device manufacturer name, if available.
    pub fn manufacturer(&self) -> String {
        if self.is_gige() {
            // SAFETY: union discriminated by nTLayerType.
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            cstr_array_to_string(&info.chManufacturerName)
        } else if self.is_usb() {
            let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
            cstr_array_to_string(&info.chManufacturerName)
        } else {
            String::new()
        }
    }

    /// Device model name.
    pub fn model(&self) -> String {
        if self.is_gige() {
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            cstr_array_to_string(&info.chModelName)
        } else if self.is_usb() {
            let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
            cstr_array_to_string(&info.chModelName)
        } else {
            String::new()
        }
    }

    /// Serial number reported by the device.
    pub fn serial(&self) -> String {
        if self.is_gige() {
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            cstr_array_to_string(&info.chSerialNumber)
        } else if self.is_usb() {
            let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
            cstr_array_to_string(&info.chSerialNumber)
        } else {
            String::new()
        }
    }

    /// User-assigned nickname (set via the MVS utility or [`Camera::set_string`]).
    pub fn user_defined_name(&self) -> String {
        if self.is_gige() {
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            cstr_array_to_string(&info.chUserDefinedName)
        } else if self.is_usb() {
            let info = unsafe { &self.raw.SpecialInfo.stUsb3VInfo };
            cstr_array_to_string(&info.chUserDefinedName)
        } else {
            String::new()
        }
    }

    /// Current IP address for GigE devices, `None` for other transports.
    pub fn ip(&self) -> Option<Ipv4Addr> {
        if self.is_gige() {
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nCurrentIp.to_be_bytes()))
        } else {
            None
        }
    }

    /// Host network interface IP for GigE devices (the NIC this device is
    /// reachable through), `None` otherwise.
    pub fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        if self.is_gige() {
            let info = unsafe { &self.raw.SpecialInfo.stGigEInfo };
            Some(Ipv4Addr::from(info.nNetExport.to_be_bytes()))
        } else {
            None
        }
    }

    /// Check whether the device can be opened in the given mode right now.
    pub fn is_accessible(&self, mode: AccessMode) -> bool {
        // SAFETY: SDK accepts a pointer to the same struct it gave us.
        let b =
            unsafe { sys::MV_CC_IsDeviceAccessible(self.raw as *const _ as *mut _, mode.raw()) };
        b != 0
    }

    /// Open this device with the given access mode. See [`open_exclusive`] /
    /// [`open_control`] for the common shortcuts.
    ///
    /// [`open_exclusive`]: Self::open_exclusive
    /// [`open_control`]: Self::open_control
    pub fn open(&self, mode: AccessMode) -> MvsResult<Camera> {
        Camera::open(self.library, self.raw, mode)
    }

    /// Shortcut for `open(AccessMode::Exclusive)`.
    pub fn open_exclusive(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Exclusive)
    }

    /// Shortcut for `open(AccessMode::Control)`.
    pub fn open_control(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Control)
    }

    /// Raw pointer to the underlying SDK struct. Intended for advanced
    /// use-cases; the pointer is valid while the parent [`DeviceList`] lives.
    pub fn as_raw(&self) -> *const sys::MV_CC_DEVICE_INFO {
        self.raw as *const _
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cstr_array_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
