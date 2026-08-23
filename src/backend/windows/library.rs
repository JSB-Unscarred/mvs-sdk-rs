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

    /// 取 `&self` 是生命周期契约：只有仍持有 `Sdk` 才能 Finalize。
    #[allow(clippy::unused_self, reason = "借用本身就是调用前提")]
    pub(crate) fn finalize(&self) -> MvsResult<()> {
        // SAFETY: Arc session owner 与 orphan handle 门禁保证相机资源已结束。
        check(unsafe { sys::MV_CC_Finalize() })
    }

    /// 返回 `Result` 以匹配 unsupported backend 的同名签名。
    #[allow(clippy::unnecessary_wraps, reason = "两个 backend 的签名必须一致")]
    pub(crate) fn sdk_version() -> MvsResult<u32> {
        // SAFETY: 官方接口允许在 Initialize 前直接查询版本。
        Ok(unsafe { sys::MV_CC_GetSDKVersion() })
    }
}
