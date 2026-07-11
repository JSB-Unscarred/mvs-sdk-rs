//! Device enumeration and per-device metadata.

use std::fmt;
use std::net::Ipv4Addr;
use std::os::raw::c_void;
use std::sync::Arc;

use crate::backend;
use crate::camera::Camera;
use crate::library::Sdk;
use crate::{AccessMode, MvsResult, TransportLayer};

/// Owned list of enumerated devices. Iterate via [`DeviceList::iter`].
pub struct DeviceList {
    inner: backend::DeviceList,
    library: Arc<Sdk>,
}

impl DeviceList {
    pub(crate) fn enumerate(library: &Arc<Sdk>, layers: TransportLayer) -> MvsResult<Self> {
        Ok(Self {
            inner: backend::DeviceList::enumerate(layers)?,
            library: Arc::clone(library),
        })
    }

    pub fn len(&self) -> usize {
        self.inner.len()
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
        self.inner.get(index).map(|inner| DeviceInfo {
            inner,
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

/// Borrowed view of one entry in a [`DeviceList`].
#[derive(Copy, Clone)]
pub struct DeviceInfo<'a> {
    inner: backend::DeviceInfo<'a>,
    library: &'a Arc<Sdk>,
}

impl DeviceInfo<'_> {
    pub fn transport_layer(&self) -> TransportLayer {
        self.inner.transport_layer()
    }

    pub fn is_gige(&self) -> bool {
        self.inner.is_gige()
    }

    pub fn is_usb(&self) -> bool {
        self.inner.is_usb()
    }

    pub fn manufacturer(&self) -> String {
        self.inner.manufacturer()
    }

    pub fn model(&self) -> String {
        self.inner.model()
    }

    pub fn serial(&self) -> String {
        self.inner.serial()
    }

    pub fn user_defined_name(&self) -> String {
        self.inner.user_defined_name()
    }

    pub fn ip(&self) -> Option<Ipv4Addr> {
        self.inner.ip()
    }

    pub fn host_nic_ip(&self) -> Option<Ipv4Addr> {
        self.inner.host_nic_ip()
    }

    pub fn is_accessible(&self, mode: AccessMode) -> bool {
        self.inner.is_accessible(mode)
    }

    pub fn open(&self, mode: AccessMode) -> MvsResult<Camera> {
        Camera::open(self.inner, self.library, mode)
    }

    pub fn open_exclusive(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Exclusive)
    }

    pub fn open_control(&self) -> MvsResult<Camera> {
        self.open(AccessMode::Control)
    }

    /// Opaque pointer to the backend device-info record. It remains valid
    /// while the parent [`DeviceList`] is alive.
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
