use std::marker::PhantomData;
use std::os::raw::c_void;
use std::slice;

use crate::error::check;
use crate::frame::{Frame, FrameInfo};
use crate::sys;
use crate::{MvsResult, PixelType};

/// 持有 `MV_CC_GetImageBuffer` 返回的 buffer，并负责一次归还。
///
/// metadata 现算自 `raw.stFrameInfo`，不另存副本。
pub(crate) struct FrameGuard<'cam> {
    raw: sys::MV_FRAME_OUT,
    handle: *mut c_void,
    _marker: PhantomData<&'cam ()>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(handle: *mut c_void, raw: sys::MV_FRAME_OUT) -> Self {
        Self {
            raw,
            handle,
            _marker: PhantomData,
        }
    }

    pub(crate) fn frame(&self) -> Frame<'_> {
        let data_len = data_len_from_raw(&self.raw.stFrameInfo);
        let data = if data_len == 0 {
            &[]
        } else {
            // SAFETY: 成功的 GetImageBuffer 按 SDK 契约返回有效 buffer 与长度。
            unsafe { slice::from_raw_parts(self.raw.pBufAddr, data_len) }
        };
        Frame::from_parts(data, self.info())
    }

    pub(crate) fn info(&self) -> FrameInfo {
        info_from_raw(&self.raw.stFrameInfo)
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        // release 消费 guard；调用前清空 handle，确保 native Free 只调用一次。
        let handle = std::mem::replace(&mut self.handle, std::ptr::null_mut());
        if handle.is_null() {
            return Ok(());
        }

        // SAFETY: handle 与 frame record 均来自同一次 GetImageBuffer。
        check(unsafe { sys::MV_CC_FreeImageBuffer(handle, &mut self.raw) })
    }
}

impl Drop for FrameGuard<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// 将 native metadata 复制为不含 SDK 指针的公共值。
pub(super) fn info_from_raw(raw: &sys::MV_FRAME_OUT_INFO_EX) -> FrameInfo {
    FrameInfo {
        width: extended_or_legacy(raw.nExtendWidth, raw.nWidth),
        height: extended_or_legacy(raw.nExtendHeight, raw.nHeight),
        pixel_type: PixelType::from_raw(raw.enPixelType as u32),
        frame_num: raw.nFrameNum,
        frame_len: frame_len_from_raw(raw),
        offset_x: raw.nOffsetX as u32,
        offset_y: raw.nOffsetY as u32,
        gain: raw.fGain,
        exposure_time: raw.fExposureTime,
        trigger_index: raw.nTriggerIndex,
        lost_packets: raw.nLostPacket,
        device_timestamp: ((raw.nDevTimeStampHigh as u64) << 32) | raw.nDevTimeStampLow as u64,
        host_timestamp_raw: raw.nHostTimeStamp,
    }
}

/// 返回 SDK 报告的有效 frame 长度；Windows x64 的 usize 可表示该字段。
pub(super) fn data_len_from_raw(raw: &sys::MV_FRAME_OUT_INFO_EX) -> usize {
    frame_len_from_raw(raw) as usize
}

fn extended_or_legacy(extended: u32, legacy: u16) -> u32 {
    if extended == 0 {
        u32::from(legacy)
    } else {
        extended
    }
}

fn frame_len_from_raw(raw: &sys::MV_FRAME_OUT_INFO_EX) -> u64 {
    if raw.nFrameLenEx == 0 {
        u64::from(raw.nFrameLen)
    } else {
        raw.nFrameLenEx
    }
}

#[cfg(test)]
mod tests {
    use crate::{PixelType, sys};

    use super::{FrameGuard, info_from_raw};

    // 验证 polling frame 使用扩展尺寸和长度，并复制核心 metadata。
    #[test]
    fn raw_frame_metadata_and_data_are_converted() {
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let raw = sys::MV_FRAME_OUT {
            pBufAddr: data.as_mut_ptr(),
            stFrameInfo: sys::MV_FRAME_OUT_INFO_EX {
                nWidth: 1,
                nHeight: 1,
                enPixelType: PixelType::MONO8.raw() as i32,
                nFrameNum: 11,
                nFrameLen: 1,
                nExtendWidth: 4,
                nExtendHeight: 2,
                nFrameLenEx: data.len() as u64,
                nOffsetX: 3,
                nOffsetY: 5,
                fGain: 1.25,
                fExposureTime: 200.5,
                nTriggerIndex: 7,
                nLostPacket: 9,
                nDevTimeStampHigh: 0x0123_4567,
                nDevTimeStampLow: 0x89AB_CDEF,
                nHostTimeStamp: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        let guard = FrameGuard::new(std::ptr::null_mut(), raw);
        let frame = guard.frame();
        let info = frame.info();

        assert_eq!(frame.data(), data);
        assert_eq!((info.width(), info.height(), info.frame_len()), (4, 2, 8));
        assert_eq!(
            (info.pixel_type(), info.frame_num()),
            (PixelType::MONO8, 11)
        );
        assert_eq!((info.offset_x(), info.offset_y()), (3, 5));
        assert_eq!((info.gain(), info.exposure_time()), (1.25, 200.5));
        assert_eq!((info.trigger_index(), info.lost_packets()), (7, 9));
        assert_eq!(info.device_timestamp(), 0x0123_4567_89AB_CDEF);
        assert_eq!(info.host_timestamp_raw(), 42);

        let legacy = sys::MV_FRAME_OUT_INFO_EX {
            nWidth: 640,
            nHeight: 480,
            nFrameLen: 307_200,
            ..Default::default()
        };
        let info = info_from_raw(&legacy);
        assert_eq!(
            (info.width(), info.height(), info.frame_len()),
            (640, 480, 307_200)
        );
    }
}
