//! Device enumeration and per-device metadata.

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;

use crate::backend;
use crate::camera::Camera;
use crate::library::Sdk;
use crate::{AccessMode, MvsResult, TransportLayer};

/// Owned list of enumerated devices. Iterate via [`DeviceList::iter`].
pub struct DeviceList {
    inner: backend::DeviceList,
}

impl DeviceList {
    pub(crate) fn enumerate(layers: TransportLayer) -> MvsResult<Self> {
        Ok(Self {
            inner: backend::DeviceList::enumerate(layers)?,
        })
    }

    /// Return the number of enumerated device snapshots.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Return whether enumeration produced no devices.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 迭代 cloned、owned device snapshot。
    pub fn iter(&self) -> impl ExactSizeIterator<Item = DeviceInfo> + '_ {
        (0..self.len()).map(|index| {
            self.get(index)
                .expect("index produced from DeviceList::len must exist")
        })
    }

    /// Clone the device snapshot at `index`, or return `None` if out of range.
    pub fn get(&self, index: usize) -> Option<DeviceInfo> {
        self.inner.get(index).map(|inner| DeviceInfo { inner })
    }
}

impl fmt::Debug for DeviceList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceList")
            .field("count", &self.len())
            .finish()
    }
}

/// Owned snapshot of one enumerated device.
#[derive(Clone)]
pub struct DeviceInfo {
    inner: backend::DeviceInfo,
}

impl DeviceInfo {
    /// Return the transport layer reported by the SDK.
    pub fn transport_layer(&self) -> TransportLayer {
        self.inner.transport_layer()
    }

    /// Return whether this is a native, virtual, or GenTL GigE device.
    pub fn is_gige(&self) -> bool {
        self.inner.is_gige()
    }

    /// Return whether this is a native or virtual USB device.
    pub fn is_usb(&self) -> bool {
        self.inner.is_usb()
    }

    /// Return the manufacturer name, decoded lossily as UTF-8.
    ///
    /// Returns an empty string when the SDK's transport record does not expose
    /// this field.
    pub fn manufacturer(&self) -> String {
        self.inner.manufacturer()
    }

    /// Return the model name, decoded lossily as UTF-8.
    ///
    /// Returns an empty string when the SDK's transport record does not expose
    /// this field.
    pub fn model(&self) -> String {
        self.inner.model()
    }

    /// Return the serial number, decoded lossily as UTF-8.
    ///
    /// Returns an empty string when the SDK's transport record does not expose
    /// this field.
    pub fn serial(&self) -> String {
        self.inner.serial()
    }

    /// Return the user-defined device name, decoded lossily as UTF-8.
    ///
    /// Returns an empty string when the SDK's transport record does not expose
    /// this field, including native Camera Link device records.
    pub fn user_defined_name(&self) -> String {
        self.inner.user_defined_name()
    }

    /// Return the current device IP for GigE devices.
    ///
    /// Other transport layers return `None`.
    pub fn ip(&self) -> Option<Ipv4Addr> {
        self.inner.ip()
    }

    /// Return the host NIC IP used by a GigE device.
    ///
    /// Other transport layers return `None`.
    pub fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        self.inner.host_nic_ip()
    }

    /// Query whether the device can currently be opened in `mode`.
    ///
    /// This requires an active process-wide [`Sdk`].
    pub fn is_accessible(&self, mode: AccessMode) -> MvsResult<bool> {
        let _active = Sdk::active()?;
        Ok(self.inner.is_accessible(mode))
    }

    /// Open this device with the requested access mode.
    ///
    /// This requires an active process-wide [`Sdk`].
    pub fn open(&self, mode: AccessMode) -> MvsResult<Camera> {
        let active = Sdk::active()?;
        Camera::open(self.inner.clone(), &active, mode)
    }

    /// Open this device with [`AccessMode::Exclusive`].
    ///
    /// See [`AccessMode`] for transport-specific behavior.
    pub fn open_exclusive(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Exclusive)
    }

    /// Open this device with [`AccessMode::Control`].
    ///
    /// See [`AccessMode`] for transport-specific behavior.
    pub fn open_control(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Control)
    }

    /// Opaque pointer to the owned backend device-info snapshot.
    ///
    /// The address remains valid while this value or one of its clones keeps
    /// the snapshot alive. After [`Sdk::shutdown`], the pointer remains valid
    /// as Rust-owned memory but must not be passed back to the native SDK.
    /// Do not mutate or free the pointed-to record.
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
