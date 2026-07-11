use std::sync::OnceLock;

use crate::MvsResult;
use crate::sys;

static INIT_RESULT: OnceLock<Result<(), i32>> = OnceLock::new();

pub(crate) struct Sdk {
    _private: (),
}

impl Sdk {
    pub(crate) fn init() -> MvsResult<Self> {
        let result = INIT_RESULT.get_or_init(|| {
            // SAFETY: SDK entry point has no arguments and is initialized once.
            let code = unsafe { sys::MV_CC_Initialize() };
            if code as u32 == sys::MV_OK {
                Ok(())
            } else {
                Err(code)
            }
        });

        match result {
            Ok(()) => Ok(Self { _private: () }),
            Err(code) => Err((*code).into()),
        }
    }

    pub(crate) fn sdk_version(&self) -> u32 {
        // SAFETY: SDK entry point has no arguments after initialization.
        unsafe { sys::MV_CC_GetSDKVersion() as u32 }
    }
}
