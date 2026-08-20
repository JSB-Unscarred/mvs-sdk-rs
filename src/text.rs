//! SDK 文本：保留原始字节，不假设编码。

use std::borrow::Cow;
use std::ffi::CString;
use std::fmt;
use std::str::Utf8Error;

use crate::{MvsError, MvsResult};

/// SDK 字符串的 owned 字节，截断于首个 NUL，不按 UTF-8 解释。
///
/// 厂商未文档化设备字符串编码；工业相机常见 GBK。需要显示时再调用
/// [`SdkText::to_str`] 或 [`SdkText::to_string_lossy`]。
#[derive(Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SdkText(Vec<u8>);

impl SdkText {
    /// 从调用方字节构造；拒绝 interior NUL，避免后续写成 C 字符串时截断。
    pub fn new(bytes: impl AsRef<[u8]>) -> MvsResult<Self> {
        let cstr = CString::new(bytes.as_ref())?;
        Ok(Self(cstr.into_bytes()))
    }

    /// 接受 FFI 已按 NUL 截断的 SDK 字段。
    pub(crate) fn from_sdk_bytes(bytes: Vec<u8>) -> Self {
        debug_assert!(!bytes.contains(&0));
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_str(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(&self.0)
    }

    #[must_use]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for SdkText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SdkText")
            .field("bytes", &self.0)
            .field("lossy", &self.to_string_lossy())
            .finish()
    }
}

impl fmt::Display for SdkText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_string_lossy())
    }
}

impl AsRef<[u8]> for SdkText {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl TryFrom<&[u8]> for SdkText {
    type Error = MvsError;

    fn try_from(value: &[u8]) -> MvsResult<Self> {
        Self::new(value)
    }
}

impl TryFrom<&str> for SdkText {
    type Error = MvsError;

    fn try_from(value: &str) -> MvsResult<Self> {
        Self::new(value.as_bytes())
    }
}

/// 按首个 NUL 截取 SDK 固定字段。
pub(crate) fn sdk_bytes_from_cstr_array(bytes: &[u8]) -> Vec<u8> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    bytes[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::SdkText;
    use crate::MvsError;

    // 验证 SDK 文本保留非 UTF-8 字节，并拒绝 interior NUL。
    #[test]
    fn text_preserves_bytes_and_rejects_nul() {
        let text = SdkText::from_sdk_bytes(vec![0x66, 0x80, 0x6F]);
        assert_eq!(text.as_bytes(), &[0x66, 0x80, 0x6F]);
        assert!(text.to_str().is_err());
        assert!(matches!(SdkText::new(b"a\0b"), Err(MvsError::Nul(_))));
    }
}
