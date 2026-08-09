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
        // SAFETY: Sdk 是唯一 owner，借用生命周期保证相机资源已结束。
        check(unsafe { sys::MV_CC_Finalize() })
    }

    pub(crate) fn sdk_version() -> MvsResult<u32> {
        // SAFETY: 官方接口允许在 Initialize 前直接查询版本。
        Ok(unsafe { sys::MV_CC_GetSDKVersion() as u32 })
    }
}
