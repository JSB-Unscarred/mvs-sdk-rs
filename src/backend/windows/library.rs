use crate::MvsResult;
use crate::error::check;
use crate::sys;

pub(crate) struct Sdk {
    _private: (),
}

impl Sdk {
    pub(crate) fn init() -> MvsResult<Self> {
        // SAFETY: process-wide serialization is provided by the safe wrapper.
        check(unsafe { sys::MV_CC_Initialize() })?;
        Ok(Self { _private: () })
    }

    pub(crate) fn finalize(&self) -> MvsResult<()> {
        // SAFETY: the process runtime holds its exclusive lifecycle gate and
        // has verified that no tracked native resource remains.
        check(unsafe { sys::MV_CC_Finalize() })
    }

    pub(crate) fn sdk_version(&self) -> u32 {
        // SAFETY: SDK entry point has no arguments after initialization.
        unsafe { sys::MV_CC_GetSDKVersion() as u32 }
    }
}
