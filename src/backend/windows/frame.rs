use std::marker::PhantomData;
use std::os::raw::c_void;
use std::slice;

use crate::error::check;
use crate::frame::{Frame, FrameInfo};
use crate::sys;
use crate::{MvsError, MvsResult, PixelType};

pub(crate) struct FrameGuard<'cam> {
    raw: sys::MV_FRAME_OUT,
    handle: *mut c_void,
    info: FrameInfo,
    data_len: usize,
    _marker: PhantomData<&'cam ()>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(handle: *mut c_void, raw: sys::MV_FRAME_OUT) -> MvsResult<Self> {
        Self::new_with(handle, raw, native_free_image_buffer)
    }

    fn new_with(
        handle: *mut c_void,
        raw: sys::MV_FRAME_OUT,
        free_invalid: impl FnOnce(*mut c_void, &mut sys::MV_FRAME_OUT) -> i32,
    ) -> MvsResult<Self> {
        let info = info_from_raw(&raw.stFrameInfo);
        let data_len = data_len_from_raw(raw.pBufAddr, &raw.stFrameInfo);
        let mut guard = Self {
            raw,
            handle,
            info,
            data_len: data_len.unwrap_or(0),
            _marker: PhantomData,
        };

        if data_len.is_none() {
            let error = MvsError::InvalidFrameBuffer {
                frame_len: info.frame_len(),
            };
            guard.release_with(free_invalid)?;
            return Err(error);
        }

        Ok(guard)
    }

    pub(crate) fn frame(&self) -> Frame<'_> {
        let data = if self.data_len == 0 {
            &[]
        } else {
            // SAFETY: the SDK buffer remains valid until this guard releases it.
            // Construction rejected a null pointer, lengths above
            // `isize::MAX`, and address ranges that wrap, as required by
            // `from_raw_parts`.
            unsafe { slice::from_raw_parts(self.raw.pBufAddr, self.data_len) }
        };
        Frame::from_parts(data, self.info)
    }

    pub(crate) fn info(&self) -> FrameInfo {
        self.info
    }

    pub(crate) fn release(&mut self) -> MvsResult<()> {
        self.release_with(native_free_image_buffer)
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

fn native_free_image_buffer(handle: *mut c_void, raw: &mut sys::MV_FRAME_OUT) -> i32 {
    // SAFETY: handle and frame record originate from GetImageBuffer.
    unsafe { sys::MV_CC_FreeImageBuffer(handle, raw) }
}

impl Drop for FrameGuard<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

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

pub(super) fn data_len_from_raw(data: *const u8, raw: &sys::MV_FRAME_OUT_INFO_EX) -> Option<usize> {
    let len = usize::try_from(frame_len_from_raw(raw)).ok()?;
    if len == 0 {
        return Some(0);
    }
    if data.is_null() || len > isize::MAX as usize {
        return None;
    }
    data.addr().checked_add(len)?;
    Some(len)
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
    use std::os::raw::c_void;

    use crate::{MvsError, PixelType, sys};

    use super::{FrameGuard, data_len_from_raw, info_from_raw};

    fn raw_frame(data: &mut [u8], frame_num: u32) -> sys::MV_FRAME_OUT {
        sys::MV_FRAME_OUT {
            pBufAddr: data.as_mut_ptr(),
            stFrameInfo: sys::MV_FRAME_OUT_INFO_EX {
                nWidth: 1,
                nHeight: 1,
                enPixelType: PixelType::MONO8.raw() as i32,
                nFrameNum: frame_num,
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
        }
    }

    // 验证 polling raw frame 的 data 与全部 metadata 字段转换正确。
    #[test]
    fn raw_frame_metadata_and_data_are_converted() {
        let mut data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let guard = FrameGuard::new(std::ptr::null_mut(), raw_frame(&mut data, 11)).unwrap();
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

    // 验证 extended width、height 和 length 不被 legacy 位宽截断。
    #[test]
    fn extended_metadata_supports_dimensions_and_lengths_above_legacy_limits() {
        let raw = sys::MV_FRAME_OUT_INFO_EX {
            nWidth: 1,
            nHeight: 2,
            nFrameLen: 3,
            nExtendWidth: 70_000,
            nExtendHeight: 80_000,
            nFrameLenEx: u64::from(u32::MAX) + 17,
            ..Default::default()
        };

        let info = info_from_raw(&raw);
        assert_eq!((info.width(), info.height()), (70_000, 80_000));
        assert_eq!(info.frame_len(), u64::from(u32::MAX) + 17);
        assert_eq!(
            data_len_from_raw(std::ptr::NonNull::<u8>::dangling().as_ptr(), &raw),
            Some(u32::MAX as usize + 17)
        );
    }

    // 验证 extended 字段为零时按 SDK 约定回退到 legacy 字段。
    #[test]
    fn legacy_metadata_is_used_when_extended_fields_are_zero() {
        let raw = sys::MV_FRAME_OUT_INFO_EX {
            nWidth: 640,
            nHeight: 480,
            nFrameLen: 307_200,
            ..Default::default()
        };

        let info = info_from_raw(&raw);
        assert_eq!((info.width(), info.height()), (640, 480));
        assert_eq!(info.frame_len(), 307_200);
        assert_eq!(
            data_len_from_raw(std::ptr::NonNull::<u8>::dangling().as_ptr(), &raw),
            Some(307_200)
        );
    }

    // 验证 null、oversized 与 wrapping buffer 在构造 slice 前被拒绝并归还。
    #[test]
    fn invalid_frame_buffers_are_rejected_without_constructing_a_slice() {
        let mut data = vec![0_u8; 1];
        let mut handle_owner = 0_u8;
        let handle = (&mut handle_owner as *mut u8).cast::<c_void>();
        let mut releases = 0;

        let mut oversized = raw_frame(&mut data, 1);
        oversized.stFrameInfo.nFrameLenEx = isize::MAX as u64 + 1;
        let Err(error) = FrameGuard::new_with(handle, oversized, |actual, _| {
            assert_eq!(actual, handle);
            releases += 1;
            sys::MV_OK as i32
        }) else {
            panic!("address-space-sized frame must be rejected");
        };
        assert!(matches!(error, MvsError::InvalidFrameBuffer { .. }));

        let mut null_buffer = raw_frame(&mut data, 2);
        null_buffer.pBufAddr = std::ptr::null_mut();
        let Err(error) = FrameGuard::new_with(handle, null_buffer, |actual, _| {
            assert_eq!(actual, handle);
            releases += 1;
            sys::MV_OK as i32
        }) else {
            panic!("non-empty null buffer must be rejected");
        };
        assert!(matches!(
            error,
            MvsError::InvalidFrameBuffer { frame_len: 1 }
        ));

        let mut wrapping = raw_frame(&mut data, 3);
        wrapping.pBufAddr = usize::MAX as *mut u8;
        let Err(error) = FrameGuard::new_with(handle, wrapping, |actual, _| {
            assert_eq!(actual, handle);
            releases += 1;
            sys::MV_OK as i32
        }) else {
            panic!("wrapping frame range must be rejected");
        };
        assert!(matches!(
            error,
            MvsError::InvalidFrameBuffer { frame_len: 1 }
        ));
        assert_eq!(releases, 3);
    }

    // 验证无效 buffer 归还失败时返回 native error，且只尝试一次。
    #[test]
    fn invalid_frame_release_failure_is_returned_without_retry() {
        let mut data = vec![0_u8; 1];
        let mut raw = raw_frame(&mut data, 1);
        raw.pBufAddr = std::ptr::null_mut();
        let mut handle_owner = 0_u8;
        let handle = (&mut handle_owner as *mut u8).cast::<c_void>();
        let mut releases = 0;

        let Err(error) = FrameGuard::new_with(handle, raw, |actual, _| {
            assert_eq!(actual, handle);
            releases += 1;
            sys::MV_E_RESOURCE as i32
        }) else {
            panic!("non-empty null buffer must be rejected");
        };

        assert!(matches!(error, MvsError::Resource));
        assert_eq!(releases, 1);
    }

    // 验证多个 guard 独立归还一次，单次失败不会触发 Drop 重试。
    #[test]
    fn two_buffers_release_once_even_when_one_release_fails() {
        let mut first_data = vec![1; 8];
        let mut second_data = vec![2; 8];
        let mut first_handle_owner = 0_u8;
        let mut second_handle_owner = 0_u8;
        let first_handle = (&mut first_handle_owner as *mut u8).cast::<c_void>();
        let second_handle = (&mut second_handle_owner as *mut u8).cast::<c_void>();
        let mut first = FrameGuard::new(first_handle, raw_frame(&mut first_data, 1)).unwrap();
        let mut second = FrameGuard::new(second_handle, raw_frame(&mut second_data, 2)).unwrap();
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
