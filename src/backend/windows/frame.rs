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
    _marker: PhantomData<&'cam ()>,
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

    pub(crate) fn info(&self) -> FrameInfo {
        FrameInfo::from_metadata(&self.metadata)
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        self.release_with(|handle, raw| {
            // SAFETY: handle and frame record originate from GetImageBuffer.
            unsafe { sys::MV_CC_FreeImageBuffer(handle, raw) }
        })
    }

    fn release_with(
        &mut self,
        free: impl FnOnce(*mut c_void, &mut sys::MV_FRAME_OUT) -> i32,
    ) -> MvsResult<()> {
        // Mark the buffer as already handled before entering the SDK. A
        // non-zero return code does not prove that the native side retained
        // ownership, so Drop must not retry the same release after an error.
        let handle = std::mem::replace(&mut self.handle, std::ptr::null_mut());
        if handle.is_null() {
            return Ok(());
        }

        check(free(handle, &mut self.raw))
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

#[cfg(test)]
mod tests {
    use std::os::raw::c_void;

    use crate::{MvsError, PixelType, sys};

    use super::FrameGuard;

    fn raw_frame(data: &mut [u8], frame_num: u32) -> sys::MV_FRAME_OUT {
        let mut raw = sys::MV_FRAME_OUT::default();
        raw.pBufAddr = data.as_mut_ptr();
        raw.stFrameInfo.nWidth = 4;
        raw.stFrameInfo.nHeight = 2;
        raw.stFrameInfo.enPixelType = PixelType::MONO8.raw() as i32;
        raw.stFrameInfo.nFrameNum = frame_num;
        raw.stFrameInfo.nFrameLen = data.len() as u32;
        raw.stFrameInfo.nOffsetX = 3;
        raw.stFrameInfo.nOffsetY = 5;
        raw.stFrameInfo.fGain = 1.25;
        raw.stFrameInfo.fExposureTime = 200.5;
        raw.stFrameInfo.nTriggerIndex = 7;
        raw.stFrameInfo.nLostPacket = 9;
        raw.stFrameInfo.nDevTimeStampHigh = 0x0123_4567;
        raw.stFrameInfo.nDevTimeStampLow = 0x89AB_CDEF;
        raw.stFrameInfo.nHostTimeStamp = 42;
        raw
    }

    #[test]
    fn raw_frame_metadata_and_data_are_converted() {
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let guard = FrameGuard::new(std::ptr::null_mut(), raw_frame(&mut data, 11));
        let frame = guard.frame();
        let info = frame.info();

        assert_eq!(frame.data(), data);
        assert_eq!((info.width(), info.height()), (4, 2));
        assert_eq!(info.pixel_type(), PixelType::MONO8);
        assert_eq!(info.frame_num(), 11);
        assert_eq!(info.frame_len(), 8);
        assert_eq!((info.offset_x(), info.offset_y()), (3, 5));
        assert_eq!((info.gain(), info.exposure_time()), (1.25, 200.5));
        assert_eq!((info.trigger_index(), info.lost_packets()), (7, 9));
        assert_eq!(info.device_timestamp(), 0x0123_4567_89AB_CDEF);
        assert_eq!(info.host_timestamp_raw(), 42);
    }

    #[test]
    fn two_buffers_release_once_even_when_one_release_fails() {
        let mut first_data = vec![1; 8];
        let mut second_data = vec![2; 8];
        let mut first_handle_owner = 0_u8;
        let mut second_handle_owner = 0_u8;
        let first_handle = (&mut first_handle_owner as *mut u8).cast::<c_void>();
        let second_handle = (&mut second_handle_owner as *mut u8).cast::<c_void>();
        let mut first = FrameGuard::new(first_handle, raw_frame(&mut first_data, 1));
        let mut second = FrameGuard::new(second_handle, raw_frame(&mut second_data, 2));
        let mut released = Vec::new();

        let error = first
            .release_with(|handle, raw| {
                released.push((handle as usize, raw.pBufAddr as usize));
                sys::MV_E_RESOURCE as i32
            })
            .unwrap_err();
        assert!(matches!(error, MvsError::Resource));
        first
            .release_with(|_, _| panic!("a failed release must not be retried"))
            .unwrap();

        second
            .release_with(|handle, raw| {
                released.push((handle as usize, raw.pBufAddr as usize));
                sys::MV_OK as i32
            })
            .unwrap();
        second
            .release_with(|_, _| panic!("a successful release must not be retried"))
            .unwrap();

        assert_eq!(released.len(), 2);
        assert_ne!(released[0].0, released[1].0);
        assert_ne!(released[0].1, released[1].1);
    }
}
