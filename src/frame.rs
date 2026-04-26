//! Image frames returned by the camera — both borrowed (`Frame`) and owned
//! (`OwnedFrame`) variants, plus the RAII `FrameGuard` used by polling mode.

use std::fmt;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::slice;
use std::time::Duration;

use crate::MvsResult;
use crate::error::check;
use crate::sys;

// ---------------------------------------------------------------------------
// PixelType
// ---------------------------------------------------------------------------

/// Wire/GVSP pixel format code. Thin newtype over the SDK's `MvGvspPixelType`.
///
/// Only the most commonly used formats are given named constants. For any
/// other format, construct via [`PixelType::from_raw`] or compare against
/// [`PixelType::raw`].
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PixelType(u32);

impl PixelType {
    pub const UNDEFINED: Self = Self(0xFFFF_FFFF);

    // Mono
    pub const MONO8: Self = Self(0x0108_0001);
    pub const MONO10: Self = Self(0x0110_0003);
    pub const MONO10_PACKED: Self = Self(0x010C_0004);
    pub const MONO12: Self = Self(0x0110_0005);
    pub const MONO12_PACKED: Self = Self(0x010C_0006);
    pub const MONO14: Self = Self(0x0110_0025);
    pub const MONO16: Self = Self(0x0110_0007);

    // Bayer 8-bit
    pub const BAYER_GR8: Self = Self(0x0108_0008);
    pub const BAYER_RG8: Self = Self(0x0108_0009);
    pub const BAYER_GB8: Self = Self(0x0108_000A);
    pub const BAYER_BG8: Self = Self(0x0108_000B);

    // Packed RGB
    pub const RGB8_PACKED: Self = Self(0x0218_0014);
    pub const BGR8_PACKED: Self = Self(0x0218_0015);
    pub const RGBA8_PACKED: Self = Self(0x0220_0016);
    pub const BGRA8_PACKED: Self = Self(0x0220_0017);

    // YUV
    pub const YUV422_PACKED: Self = Self(0x0210_001F);
    pub const YUV422_YUYV_PACKED: Self = Self(0x0210_0032);

    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Effective bits per pixel encoded in the format descriptor (e.g. 8, 24, 48).
    #[inline]
    pub const fn bits_per_pixel(self) -> u32 {
        (self.0 >> 16) & 0xFF
    }

    #[inline]
    pub const fn is_mono(self) -> bool {
        (self.0 & 0xFF00_0000) == 0x0100_0000
    }

    #[inline]
    pub const fn is_color(self) -> bool {
        (self.0 & 0xFF00_0000) == 0x0200_0000
    }

    #[inline]
    pub const fn is_custom(self) -> bool {
        (self.0 & 0x8000_0000) != 0
    }
}

impl fmt::Debug for PixelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PixelType(0x{:08X})", self.0)
    }
}

// ---------------------------------------------------------------------------
// FrameInfo
// ---------------------------------------------------------------------------

/// Metadata for an image frame. Borrowed view over the raw SDK struct.
#[derive(Copy, Clone)]
pub struct FrameInfo<'a>(&'a sys::MV_FRAME_OUT_INFO_EX);

impl<'a> FrameInfo<'a> {
    pub fn width(&self) -> u32 {
        self.0.nWidth as u32
    }
    pub fn height(&self) -> u32 {
        self.0.nHeight as u32
    }
    pub fn pixel_type(&self) -> PixelType {
        PixelType::from_raw(self.0.enPixelType as u32)
    }
    pub fn frame_num(&self) -> u32 {
        self.0.nFrameNum
    }
    pub fn frame_len(&self) -> u32 {
        self.0.nFrameLen
    }
    pub fn offset_x(&self) -> u32 {
        self.0.nOffsetX as u32
    }
    pub fn offset_y(&self) -> u32 {
        self.0.nOffsetY as u32
    }
    pub fn gain(&self) -> f32 {
        self.0.fGain
    }
    pub fn exposure_time(&self) -> f32 {
        self.0.fExposureTime
    }
    pub fn trigger_index(&self) -> u32 {
        self.0.nTriggerIndex
    }
    pub fn lost_packets(&self) -> u32 {
        self.0.nLostPacket
    }

    /// Device-reported timestamp as a single 64-bit tick count.
    pub fn device_timestamp(&self) -> u64 {
        ((self.0.nDevTimeStampHigh as u64) << 32) | self.0.nDevTimeStampLow as u64
    }

    /// Host-reported timestamp at arrival (SDK convention: 100-ns ticks since
    /// some host-specific epoch). Treat as opaque unless you know what your
    /// SDK version returns here.
    pub fn host_timestamp_raw(&self) -> i64 {
        self.0.nHostTimeStamp
    }

    /// Host timestamp interpreted as a `Duration` since the epoch chosen by
    /// the SDK (assumes 100-ns ticks, which is the Windows convention).
    pub fn host_timestamp(&self) -> Duration {
        let ticks = self.0.nHostTimeStamp.max(0) as u64;
        Duration::from_nanos(ticks * 100)
    }
}

impl fmt::Debug for FrameInfo<'_> {
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

// ---------------------------------------------------------------------------
// Frame (borrowed)
// ---------------------------------------------------------------------------

/// A borrowed view of an image frame. The pixel data is valid only for the
/// lifetime `'a` — inside an image callback this is the callback scope; for
/// [`FrameGuard`] it is the guard's scope. To keep data around, call
/// [`Frame::to_owned`].
pub struct Frame<'a> {
    data: &'a [u8],
    info: FrameInfo<'a>,
}

impl<'a> Frame<'a> {
    pub(crate) unsafe fn from_raw_parts(
        data_ptr: *const u8,
        info: &'a sys::MV_FRAME_OUT_INFO_EX,
    ) -> Self {
        let len = info.nFrameLen as usize;
        // SAFETY: caller asserts data_ptr is valid for `len` bytes with lifetime `'a`.
        let data = if data_ptr.is_null() || len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(data_ptr, len) }
        };
        Self {
            data,
            info: FrameInfo(info),
        }
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        self.data
    }

    #[inline]
    pub fn info(&self) -> &FrameInfo<'a> {
        &self.info
    }

    /// Copy the frame into an owned, `Send + 'static` buffer.
    pub fn to_owned(&self) -> OwnedFrame {
        OwnedFrame {
            data: self.data.to_vec(),
            info: *self.info.0,
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

// ---------------------------------------------------------------------------
// OwnedFrame
// ---------------------------------------------------------------------------

/// An owned image frame: independent of any SDK buffer, safe to send across
/// threads and keep indefinitely.
#[derive(Clone)]
pub struct OwnedFrame {
    /// Raw pixel bytes in the format indicated by [`FrameInfo::pixel_type`].
    pub data: Vec<u8>,
    info: sys::MV_FRAME_OUT_INFO_EX,
}

impl OwnedFrame {
    pub fn info(&self) -> FrameInfo<'_> {
        FrameInfo(&self.info)
    }

    pub fn as_frame(&self) -> Frame<'_> {
        Frame {
            data: &self.data,
            info: FrameInfo(&self.info),
        }
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

// ---------------------------------------------------------------------------
// FrameGuard (polling mode)
// ---------------------------------------------------------------------------

/// RAII guard returned by [`Camera::get_image_buffer`]. Drops call
/// `MV_CC_FreeImageBuffer` on the underlying buffer. The frame data is
/// borrowed from the SDK — call [`FrameGuard::to_owned`] to detach it.
///
/// [`Camera::get_image_buffer`]: crate::Camera::get_image_buffer
pub struct FrameGuard<'cam> {
    raw: sys::MV_FRAME_OUT,
    handle: *mut c_void,
    _marker: PhantomData<&'cam mut ()>,
}

impl<'cam> FrameGuard<'cam> {
    pub(crate) fn new(handle: *mut c_void, raw: sys::MV_FRAME_OUT) -> Self {
        Self {
            raw,
            handle,
            _marker: PhantomData,
        }
    }

    pub fn frame(&self) -> Frame<'_> {
        // SAFETY: the buffer is valid until this guard is dropped; info lives
        // inside `self.raw` which is pinned by `self`.
        unsafe { Frame::from_raw_parts(self.raw.pBufAddr, &self.raw.stFrameInfo) }
    }

    pub fn info(&self) -> FrameInfo<'_> {
        FrameInfo(&self.raw.stFrameInfo)
    }

    pub fn to_owned(&self) -> OwnedFrame {
        self.frame().to_owned()
    }

    /// Free the buffer eagerly, returning any SDK error. If you don't call
    /// this, the buffer is freed on drop and errors are ignored.
    pub fn release(mut self) -> MvsResult<()> {
        // SAFETY: same handle that produced the buffer.
        let code = unsafe { sys::MV_CC_FreeImageBuffer(self.handle, &mut self.raw) };
        // Prevent Drop from calling FreeImageBuffer again.
        self.handle = std::ptr::null_mut();
        check(code)
    }
}

impl<'cam> Drop for FrameGuard<'cam> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: handle + raw are paired from MV_CC_GetImageBuffer. Ignore
            // error because Drop cannot propagate.
            unsafe {
                let _ = sys::MV_CC_FreeImageBuffer(self.handle, &mut self.raw);
            }
        }
    }
}
