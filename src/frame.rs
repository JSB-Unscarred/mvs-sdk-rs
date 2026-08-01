//! Platform-independent image frame views and ownership types.

use std::fmt;
use std::time::Duration;

use crate::backend;
use crate::{MvsResult, PixelType};

#[derive(Copy, Clone)]
pub(crate) struct FrameMetadata {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixel_type: PixelType,
    pub(crate) frame_num: u32,
    pub(crate) frame_len: u32,
    pub(crate) offset_x: u32,
    pub(crate) offset_y: u32,
    pub(crate) gain: f32,
    pub(crate) exposure_time: f32,
    pub(crate) trigger_index: u32,
    pub(crate) lost_packets: u32,
    pub(crate) device_timestamp: u64,
    pub(crate) host_timestamp_raw: i64,
}

/// Metadata for an image frame.
#[derive(Copy, Clone)]
pub struct FrameInfo(FrameMetadata);

impl FrameInfo {
    pub(crate) fn from_metadata(metadata: &FrameMetadata) -> Self {
        Self(*metadata)
    }

    pub fn width(&self) -> u32 {
        self.0.width
    }

    pub fn height(&self) -> u32 {
        self.0.height
    }

    pub fn pixel_type(&self) -> PixelType {
        self.0.pixel_type
    }

    pub fn frame_num(&self) -> u32 {
        self.0.frame_num
    }

    pub fn frame_len(&self) -> u32 {
        self.0.frame_len
    }

    pub fn offset_x(&self) -> u32 {
        self.0.offset_x
    }

    pub fn offset_y(&self) -> u32 {
        self.0.offset_y
    }

    pub fn gain(&self) -> f32 {
        self.0.gain
    }

    pub fn exposure_time(&self) -> f32 {
        self.0.exposure_time
    }

    pub fn trigger_index(&self) -> u32 {
        self.0.trigger_index
    }

    pub fn lost_packets(&self) -> u32 {
        self.0.lost_packets
    }

    pub fn device_timestamp(&self) -> u64 {
        self.0.device_timestamp
    }

    pub fn host_timestamp_raw(&self) -> i64 {
        self.0.host_timestamp_raw
    }

    pub fn host_timestamp(&self) -> Duration {
        let ticks = self.0.host_timestamp_raw.max(0) as u64;
        Duration::from_nanos(ticks.saturating_mul(100))
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
    pub(crate) fn from_parts(data: &'a [u8], metadata: &FrameMetadata) -> Self {
        Self {
            data,
            info: FrameInfo::from_metadata(metadata),
        }
    }

    pub fn data(&self) -> &[u8] {
        self.data
    }

    pub fn info(&self) -> FrameInfo {
        self.info
    }

    pub fn to_owned(&self) -> OwnedFrame {
        OwnedFrame {
            data: self.data.to_vec(),
            info: self.info.0,
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
    /// Raw pixel bytes in the format indicated by [`FrameInfo::pixel_type`].
    pub data: Vec<u8>,
    info: FrameMetadata,
}

impl OwnedFrame {
    pub fn info(&self) -> FrameInfo {
        FrameInfo::from_metadata(&self.info)
    }

    pub fn as_frame(&self) -> Frame<'_> {
        Frame::from_parts(&self.data, &self.info)
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
pub struct FrameGuard<'cam> {
    inner: backend::FrameGuard<'cam>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(inner: backend::FrameGuard<'cam>) -> Self {
        Self { inner }
    }

    pub fn frame(&self) -> Frame<'_> {
        self.inner.frame()
    }

    pub fn info(&self) -> FrameInfo {
        self.inner.info()
    }

    pub fn to_owned(&self) -> OwnedFrame {
        self.frame().to_owned()
    }

    pub fn release(mut self) -> MvsResult<()> {
        self.inner.release()
    }
}

impl Drop for FrameGuard<'_> {
    fn drop(&mut self) {
        let _ = self.inner.release();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FrameInfo, FrameMetadata, OwnedFrame};
    use crate::PixelType;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn owned_frame_is_send_and_sync() {
        assert_send_sync::<OwnedFrame>();
    }

    #[test]
    fn frame_info_owns_its_metadata_snapshot() {
        let info = {
            let metadata = FrameMetadata {
                width: 640,
                height: 480,
                pixel_type: PixelType::MONO8,
                frame_num: 42,
                frame_len: 307_200,
                offset_x: 3,
                offset_y: 4,
                gain: 1.5,
                exposure_time: 250.0,
                trigger_index: 7,
                lost_packets: 2,
                device_timestamp: 99,
                host_timestamp_raw: 123,
            };
            FrameInfo::from_metadata(&metadata)
        };

        assert_eq!(info.width(), 640);
        assert_eq!(info.height(), 480);
        assert_eq!(info.pixel_type(), PixelType::MONO8);
        assert_eq!(info.frame_num(), 42);
        assert_eq!(info.host_timestamp(), Duration::from_nanos(12_300));
    }
}
