//! Platform-independent image frame views and ownership types.

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::backend;
use crate::{MvsResult, PixelType};

/// Metadata for an image frame.
#[derive(Copy, Clone)]
pub struct FrameInfo {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixel_type: PixelType,
    pub(crate) frame_num: u32,
    pub(crate) frame_len: u64,
    pub(crate) offset_x: u32,
    pub(crate) offset_y: u32,
    pub(crate) gain: f32,
    pub(crate) exposure_time: f32,
    pub(crate) trigger_index: u32,
    pub(crate) lost_packets: u32,
    pub(crate) device_timestamp: u64,
    pub(crate) host_timestamp_raw: i64,
}

impl FrameInfo {
    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format reported by the SDK.
    pub fn pixel_type(&self) -> PixelType {
        self.pixel_type
    }

    /// Device frame sequence number.
    pub fn frame_num(&self) -> u32 {
        self.frame_num
    }

    /// Number of valid bytes in the image buffer.
    ///
    /// Native frames use the SDK's extended 64-bit length when available.
    pub fn frame_len(&self) -> u64 {
        self.frame_len
    }

    /// Horizontal image-region offset in pixels.
    pub fn offset_x(&self) -> u32 {
        self.offset_x
    }

    /// Vertical image-region offset in pixels.
    pub fn offset_y(&self) -> u32 {
        self.offset_y
    }

    /// Gain recorded in the frame metadata.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Exposure time recorded in the frame metadata.
    pub fn exposure_time(&self) -> f32 {
        self.exposure_time
    }

    /// Trigger sequence index reported by the device.
    pub fn trigger_index(&self) -> u32 {
        self.trigger_index
    }

    /// Number of lost packets reported for this frame.
    pub fn lost_packets(&self) -> u32 {
        self.lost_packets
    }

    /// Device timestamp assembled from the SDK's high and low words.
    pub fn device_timestamp(&self) -> u64 {
        self.device_timestamp
    }

    /// Raw signed host timestamp returned by the SDK.
    ///
    /// The installed SDK headers do not define this value's unit, so the
    /// wrapper intentionally leaves interpretation to the application.
    pub fn host_timestamp_raw(&self) -> i64 {
        self.host_timestamp_raw
    }
}

impl fmt::Debug for FrameInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameInfo")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("pixel_type", &self.pixel_type())
            .field("frame_num", &self.frame_num())
            .field("frame_len", &self.frame_len())
            .finish()
    }
}

/// A borrowed image frame. To keep it beyond the callback or guard lifetime,
/// call [`Frame::to_owned`].
pub struct Frame<'a> {
    data: &'a [u8],
    info: FrameInfo,
}

impl<'a> Frame<'a> {
    pub(crate) fn from_parts(data: &'a [u8], info: FrameInfo) -> Self {
        Self { data, info }
    }

    /// Borrow the valid pixel bytes for this frame.
    pub fn data(&self) -> &[u8] {
        self.data
    }

    /// Return a copy of this frame's metadata.
    pub fn info(&self) -> FrameInfo {
        self.info
    }

    /// Copy the pixels and metadata into SDK-independent storage.
    pub fn to_owned(&self) -> OwnedFrame {
        let mut info = self.info;
        info.frame_len = self.data.len() as u64;
        OwnedFrame {
            data: self.data.to_vec(),
            info,
        }
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("info", &self.info)
            .field("data.len", &self.data.len())
            .finish()
    }
}

/// An owned image frame, independent of any SDK buffer.
#[derive(Clone)]
pub struct OwnedFrame {
    data: Vec<u8>,
    info: FrameInfo,
}

impl OwnedFrame {
    /// Borrow the owned pixel bytes in the format indicated by
    /// [`FrameInfo::pixel_type`].
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutably borrow the owned pixel bytes without changing their length.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Consume the frame and return its pixel allocation.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Return a copy of the owned frame's metadata.
    pub fn info(&self) -> FrameInfo {
        self.info
    }

    /// Borrow this owned allocation as a [`Frame`].
    pub fn as_frame(&self) -> Frame<'_> {
        Frame::from_parts(&self.data, self.info)
    }
}

impl fmt::Debug for OwnedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedFrame")
            .field("info", &self.info())
            .field("data.len", &self.data.len())
            .finish()
    }
}

/// RAII guard returned by [`Camera::get_image_buffer`](crate::Camera::get_image_buffer).
///
/// The guard keeps the camera borrowed and the SDK buffer valid. It is neither
/// `Send` nor `Sync`; inspect, copy, and release it on the acquiring thread.
/// Dropping the guard makes one best-effort release attempt and cannot report
/// an error, so use [`FrameGuard::release`] when release failures matter.
pub struct FrameGuard<'cam> {
    inner: backend::FrameGuard<'cam>,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(inner: backend::FrameGuard<'cam>) -> Self {
        Self {
            inner,
            _not_send_sync: PhantomData,
        }
    }

    /// Borrow the guarded SDK buffer as a frame.
    pub fn frame(&self) -> Frame<'_> {
        self.inner.frame()
    }

    /// Return a copy of the guarded frame's metadata without borrowing pixels.
    pub fn info(&self) -> FrameInfo {
        self.inner.info()
    }

    /// Copy the guarded frame into SDK-independent storage.
    pub fn to_owned(&self) -> OwnedFrame {
        self.frame().to_owned()
    }

    /// Release the SDK buffer and report the vendor result.
    ///
    /// The guard is consumed, so the native release is attempted exactly once.
    pub fn release(mut self) -> MvsResult<()> {
        self.inner.release()
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, FrameInfo};
    use crate::PixelType;

    // 验证 frame copy 会按实际 data 长度校正 metadata，并与 SDK buffer 解耦。
    #[test]
    fn owned_frame_keeps_data_length_and_metadata_consistent() {
        let info = FrameInfo {
            width: 2,
            height: 1,
            pixel_type: PixelType::MONO8,
            frame_num: 1,
            frame_len: 99,
            offset_x: 0,
            offset_y: 0,
            gain: 0.0,
            exposure_time: 0.0,
            trigger_index: 0,
            lost_packets: 0,
            device_timestamp: 0,
            host_timestamp_raw: 0,
        };

        let mut owned = Frame::from_parts(&[1, 2], info).to_owned();
        assert_eq!(owned.info().frame_len(), 2);
        owned.data_mut()[1] = 3;
        assert_eq!(owned.as_frame().data(), [1, 3]);
        assert_eq!(owned.into_data(), [1, 3]);
    }
}
