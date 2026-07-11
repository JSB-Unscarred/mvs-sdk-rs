use std::marker::PhantomData;
use std::os::raw::c_void;
use std::slice;

use crate::error::check;
use crate::frame::{Frame, FrameInfo, FrameMetadata};
use crate::sys;
use crate::{MvsResult, PixelType};

pub(crate) struct FrameGuard<'cam> {
    raw: sys::MV_FRAME_OUT,
    handle: *mut c_void,
    metadata: FrameMetadata,
    _marker: PhantomData<&'cam mut ()>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(handle: *mut c_void, raw: sys::MV_FRAME_OUT) -> Self {
        let metadata = metadata_from_raw(&raw.stFrameInfo);
        Self {
            raw,
            handle,
            metadata,
            _marker: PhantomData,
        }
    }

    pub(crate) fn frame(&self) -> Frame<'_> {
        let len = self.raw.stFrameInfo.nFrameLen as usize;
        let data = if self.raw.pBufAddr.is_null() || len == 0 {
            &[]
        } else {
            // SAFETY: the SDK buffer remains valid until this guard releases it.
            unsafe { slice::from_raw_parts(self.raw.pBufAddr, len) }
        };
        Frame::from_parts(data, &self.metadata)
    }

    pub(crate) fn info(&self) -> FrameInfo<'_> {
        FrameInfo::from_metadata(&self.metadata)
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        if self.handle.is_null() {
            return Ok(());
        }

        // SAFETY: handle and frame record originate from GetImageBuffer.
        let code = unsafe { sys::MV_CC_FreeImageBuffer(self.handle, &mut self.raw) };
        check(code)?;
        self.handle = std::ptr::null_mut();
        Ok(())
    }
}

pub(super) fn metadata_from_raw(raw: &sys::MV_FRAME_OUT_INFO_EX) -> FrameMetadata {
    FrameMetadata {
        width: raw.nWidth as u32,
        height: raw.nHeight as u32,
        pixel_type: PixelType::from_raw(raw.enPixelType as u32),
        frame_num: raw.nFrameNum,
        frame_len: raw.nFrameLen,
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
