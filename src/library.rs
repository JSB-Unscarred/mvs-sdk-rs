//! SDK lifetime — one-shot init, wrapped in an [`Arc`]-countable handle so
//! every [`Camera`] can keep the SDK alive for its own lifetime.
//!
//! `MV_CC_Finalize` is intentionally **not** called from [`Sdk::drop`].
//! The SDK does not reliably support re-initialization within a single
//! process, so we initialize once and let process exit handle cleanup.
//!
//! [`Camera`]: crate::Camera

use std::sync::{Arc, OnceLock};

use crate::MvsResult;
use crate::device::{DeviceList, TransportLayer};
use crate::sys;

static INIT_RESULT: OnceLock<Result<(), i32>> = OnceLock::new();

/// Handle to the initialized MVS SDK. Obtain one with [`Sdk::init`] and
/// share via [`Arc::clone`]. Calling [`Sdk::init`] multiple times is
/// cheap: the SDK is initialized exactly once per process.
pub struct Sdk {
    _private: (),
}

impl Sdk {
    /// Initialize the MVS SDK (idempotent across the process).
    pub fn init() -> MvsResult<Arc<Self>> {
        let result = INIT_RESULT.get_or_init(|| {
            // SAFETY: SDK entry point, no arguments.
            let code = unsafe { sys::MV_CC_Initialize() };
            if code as u32 == sys::MV_OK {
                Ok(())
            } else {
                Err(code)
            }
        });
        match result {
            Ok(()) => Ok(Arc::new(Self { _private: () })),
            Err(code) => Err((*code).into()),
        }
    }

    /// SDK version as a packed `u32`; interpret per MVS SDK documentation.
    pub fn sdk_version(&self) -> u32 {
        // SAFETY: SDK entry point, no arguments.
        unsafe { sys::MV_CC_GetSDKVersion() as u32 }
    }

    /// Enumerate connected devices of the given transport types. Pass
    /// `TransportLayer::GIGE | TransportLayer::USB` for the common case.
    pub fn enumerate_devices(self: &Arc<Self>, layers: TransportLayer) -> MvsResult<DeviceList> {
        DeviceList::enumerate(self, layers)
    }
}
