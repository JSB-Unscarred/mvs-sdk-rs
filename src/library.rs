//! SDK lifetime and initialization.

use std::sync::Arc;

use crate::backend;
use crate::device::DeviceList;
use crate::{MvsResult, TransportLayer};

/// Handle to the initialized MVS SDK. Calling [`Sdk::init`] multiple times is
/// cheap: the native SDK is initialized exactly once per process.
pub struct Sdk {
    pub(crate) inner: backend::Sdk,
}

impl Sdk {
    /// Initialize the MVS SDK.
    pub fn init() -> MvsResult<Arc<Self>> {
        backend::Sdk::init().map(|inner| Arc::new(Self { inner }))
    }

    /// SDK version as a packed `u32`; interpret per MVS SDK documentation.
    pub fn sdk_version(&self) -> u32 {
        self.inner.sdk_version()
    }

    /// Enumerate connected devices of the requested transport types.
    pub fn enumerate_devices(self: &Arc<Self>, layers: TransportLayer) -> MvsResult<DeviceList> {
        DeviceList::enumerate(self, layers)
    }
}
