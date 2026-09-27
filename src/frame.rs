//! 图像帧：借用 SDK buffer 的视图，以及 polling buffer 的归还守卫。

use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::slice;

use crate::{PixelType, high_low, sys};

/// 帧的元数据，复制自 `MV_FRAME_OUT_INFO_EX`。
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FrameInfo {
    /// 宽度，优先取扩展字段。
    pub width: u32,
    /// 高度，优先取扩展字段。
    pub height: u32,
    /// 像素格式。
    pub pixel_type: PixelType,
    /// 帧号。
    pub frame_number: u32,
    /// 水平偏移。
    pub offset_x: u32,
    /// 垂直偏移。
    pub offset_y: u32,
    /// 增益。
    pub gain: f32,
    /// 曝光时间。
    pub exposure_time: f32,
    /// 触发计数。
    pub trigger_index: u32,
    /// 丢包数。
    pub lost_packets: u32,
    /// 设备时间戳。
    pub device_timestamp: u64,
    /// 主机时间戳；头文件未定义单位。
    pub host_timestamp: i64,
}

/// 借用 SDK buffer 的一帧图像。
///
/// callback 中的帧只在本次回调期间有效，polling 帧不能超过其 [`FrameGuard`]；
/// 需要保留像素时复制 [`Frame::data`]。
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    data: &'a [u8],
    info: FrameInfo,
}

impl<'a> Frame<'a> {
    /// # Safety
    ///
    /// `raw` 来自成功的 `GetImageBuffer` 或 image callback，像素在 `'a` 内有效。
    #[allow(clippy::cast_possible_truncation, reason = "只支持 64 位 Windows")]
    pub(crate) unsafe fn from_raw(raw: &'a sys::MV_FRAME_OUT) -> Self {
        let info = &raw.stFrameInfo;
        let len = if info.nFrameLenEx == 0 {
            u64::from(info.nFrameLen)
        } else {
            info.nFrameLenEx
        };
        let data = if raw.pBufAddr.is_null() || len == 0 {
            &[]
        } else {
            // SAFETY: 调用方保证 pBufAddr 指向 len 字节的有效像素。
            unsafe { slice::from_raw_parts(raw.pBufAddr, len as usize) }
        };
        Self {
            data,
            info: FrameInfo {
                width: extended_or(info.nExtendWidth, info.nWidth),
                height: extended_or(info.nExtendHeight, info.nHeight),
                pixel_type: PixelType::from_raw(info.enPixelType.cast_unsigned()),
                frame_number: info.nFrameNum,
                offset_x: u32::from(info.nOffsetX),
                offset_y: u32::from(info.nOffsetY),
                gain: info.fGain,
                exposure_time: info.fExposureTime,
                trigger_index: info.nTriggerIndex,
                lost_packets: info.nLostPacket,
                device_timestamp: high_low(info.nDevTimeStampHigh, info.nDevTimeStampLow),
                host_timestamp: info.nHostTimeStamp,
            },
        }
    }

    /// 像素字节，格式见 [`FrameInfo::pixel_type`]。
    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// 帧的元数据。
    pub const fn info(&self) -> &FrameInfo {
        &self.info
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("info", &self.info)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// polling 取得的 SDK buffer，释放时调用 `MV_CC_FreeImageBuffer` 归还。
///
/// 守卫借用 [`Grabbing`](crate::Grabbing)，因此 buffer 必然在停止取流前归还。
pub struct FrameGuard<'a> {
    raw: sys::MV_FRAME_OUT,
    handle: *mut c_void,
    _grabbing: PhantomData<&'a ()>,
}

impl FrameGuard<'_> {
    pub(crate) const fn new(handle: *mut c_void, raw: &sys::MV_FRAME_OUT) -> Self {
        Self {
            raw: *raw,
            handle,
            _grabbing: PhantomData,
        }
    }

    /// 借出 buffer 中的帧。
    pub fn frame(&self) -> Frame<'_> {
        // SAFETY: raw 来自成功的 GetImageBuffer，buffer 在守卫释放前有效。
        unsafe { Frame::from_raw(&self.raw) }
    }
}

impl Drop for FrameGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: handle 与 raw 来自同一次 GetImageBuffer，只归还一次。
        unsafe { sys::MV_CC_FreeImageBuffer(self.handle, &raw mut self.raw) };
    }
}

impl fmt::Debug for FrameGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FrameGuard").field(&self.frame()).finish()
    }
}

/// SDK 用扩展字段承载超过 `u16` 的尺寸，扩展字段为 0 时取旧字段。
fn extended_or(extended: u32, legacy: u16) -> u32 {
    if extended == 0 {
        u32::from(legacy)
    } else {
        extended
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;
    use crate::{PixelType, sys};

    // 优先使用扩展尺寸与扩展长度，缺省时回退到旧字段。
    #[test]
    fn extended_fields_take_precedence() {
        let mut pixels = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let mut raw = sys::MV_FRAME_OUT {
            pBufAddr: pixels.as_mut_ptr(),
            stFrameInfo: sys::MV_FRAME_OUT_INFO_EX {
                nWidth: 1,
                nExtendWidth: 4,
                nHeight: 2,
                nFrameLen: 1,
                nFrameLenEx: 8,
                enPixelType: PixelType::MONO8.raw().cast_signed(),
                nDevTimeStampHigh: 1,
                nDevTimeStampLow: 2,
                ..Default::default()
            },
            ..Default::default()
        };

        // SAFETY: pixels 覆盖 raw 声明的 8 字节。
        let frame = unsafe { Frame::from_raw(&raw) };
        assert_eq!((frame.info().width, frame.info().height), (4, 2));
        assert_eq!(frame.data(), pixels);
        assert_eq!(frame.info().device_timestamp, 0x1_0000_0002);

        raw.stFrameInfo.nFrameLenEx = 0;
        // SAFETY: 同上，长度缩短为 1 字节。
        assert_eq!(unsafe { Frame::from_raw(&raw) }.data(), [1]);
    }
}
