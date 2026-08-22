//! Platform-independent public value types.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::sys;

/// 取图等待时长。
///
/// SDK 用 `u32::MAX` 表示无限等待，本类型把该哨兵与有限毫秒分开表达，
/// 非法组合在编译期不可构造。
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Timeout {
    /// 有限等待毫秒数。`u32::MAX` 归一到最大有限值，防止退化成无限等待。
    Finite(u32),
    /// 等待到有帧或出错为止。
    Infinite,
}

impl Timeout {
    /// 映射到 SDK 的等待参数；无限等待哨兵只在此处产生。
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Finite(milliseconds) => milliseconds.min(u32::MAX - 1),
            Self::Infinite => u32::MAX,
        }
    }
}

/// Device access mode passed to [`Sdk::open`](crate::Sdk::open).
///
/// The vendor SDK applies these modes differently by transport. The mode and
/// switchover key are meaningful for native GigE devices, although current
/// firmware may restrict the switchover variants. GenTL GigE devices accept
/// only exclusive, control, or monitor access. USB3, Camera Link, CoaXPress,
/// XoF, virtual GigE, and virtual USB devices ignore both parameters and open
/// with control access.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AccessMode {
    /// Exclusive control of the device.
    Exclusive,
    /// Exclusive control while allowing the device's control owner to switch.
    ExclusiveWithSwitch,
    /// Control access without exclusive ownership.
    Control,
    /// Control access while allowing the control owner to switch.
    ControlWithSwitch,
    /// Enable control-owner switching without a key.
    ControlSwitchEnable,
    /// Enable control-owner switching with a key supplied to `Sdk::open`.
    ControlSwitchEnableWithKey,
    /// Read-only monitoring access.
    Monitor,
}

/// Integer node value snapshot: current value plus its allowed range.
#[derive(Copy, Clone, Debug)]
pub struct IntValue {
    /// Current node value.
    pub current: i64,
    /// Smallest accepted value.
    pub min: i64,
    /// Largest accepted value.
    pub max: i64,
    /// Required increment between accepted values.
    pub inc: i64,
}

/// Float node value snapshot: current value plus min/max.
#[derive(Copy, Clone, Debug)]
pub struct FloatValue {
    /// Current node value.
    pub current: f32,
    /// Smallest accepted value.
    pub min: f32,
    /// Largest accepted value.
    pub max: f32,
}

/// Enum node value snapshot: current numeric value and allowed values.
#[derive(Clone, Debug)]
pub struct EnumValue {
    /// Current numeric value.
    pub current: u32,
    /// Numeric values reported as supported by the node.
    pub supported: Vec<u32>,
}

/// Bit set of transport-layer protocols to enumerate. Combine with `|`.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct TransportLayer(u32);

impl TransportLayer {
    /// Unknown or unspecified transport.
    pub const UNKNOWN: Self = Self(sys::MV_UNKNOW_DEVICE);
    /// GigE Vision devices.
    pub const GIGE: Self = Self(sys::MV_GIGE_DEVICE);
    /// IEEE 1394-a/b devices.
    pub const IEEE_1394: Self = Self(sys::MV_1394_DEVICE);
    /// USB3 Vision devices.
    pub const USB: Self = Self(sys::MV_USB_DEVICE);
    /// Camera Link devices.
    pub const CAMERALINK: Self = Self(sys::MV_CAMERALINK_DEVICE);
    /// Virtual GigE devices.
    pub const VIR_GIGE: Self = Self(sys::MV_VIR_GIGE_DEVICE);
    /// Virtual USB devices.
    pub const VIR_USB: Self = Self(sys::MV_VIR_USB_DEVICE);
    /// GigE devices exposed through GenTL.
    pub const GENTL_GIGE: Self = Self(sys::MV_GENTL_GIGE_DEVICE);
    /// Camera Link devices exposed through GenTL.
    pub const GENTL_CAMERALINK: Self = Self(sys::MV_GENTL_CAMERALINK_DEVICE);
    /// CoaXPress devices exposed through GenTL.
    pub const GENTL_CXP: Self = Self(sys::MV_GENTL_CXP_DEVICE);
    /// XoF devices exposed through GenTL.
    pub const GENTL_XOF: Self = Self(sys::MV_GENTL_XOF_DEVICE);
    /// Virtual devices exposed through GenTL.
    pub const GENTL_VIR: Self = Self(sys::MV_GENTL_VIR_DEVICE);

    /// Every transport-layer bit defined by the SDK header version used to
    /// generate these bindings.
    pub const ALL: Self = Self(
        Self::GIGE.0
            | Self::IEEE_1394.0
            | Self::USB.0
            | Self::CAMERALINK.0
            | Self::VIR_GIGE.0
            | Self::VIR_USB.0
            | Self::GENTL_GIGE.0
            | Self::GENTL_CAMERALINK.0
            | Self::GENTL_CXP.0
            | Self::GENTL_XOF.0
            | Self::GENTL_VIR.0,
    );

    /// Construct a transport-layer bit set from the SDK's raw mask.
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the SDK transport-layer mask.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Return whether every bit in `other` is present in this set.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// 是否包含 GigE 系 transport，含虚拟设备与 GenTL 设备。
    ///
    /// 位集语义。设备快照的 `nTLayerType` 由 SDK 写为单一 device type 值，
    /// 因此对 [`crate::DeviceProperties::transport_layer`] 与逐值比较等价。
    #[inline]
    pub const fn is_gige(self) -> bool {
        self.contains(Self::GIGE)
            || self.contains(Self::VIR_GIGE)
            || self.contains(Self::GENTL_GIGE)
    }

    /// 是否包含 USB 系 transport，含虚拟设备。
    #[inline]
    pub const fn is_usb(self) -> bool {
        self.contains(Self::USB) || self.contains(Self::VIR_USB)
    }
}

impl BitOr for TransportLayer {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for TransportLayer {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for TransportLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransportLayer(0x{:08X})", self.0)
    }
}

/// Wire/GVSP pixel format code. Thin newtype over the SDK's pixel-type value.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PixelType(u32);

impl PixelType {
    // The vendor's custom bit is orthogonal to the mono/color category bits.
    const CATEGORY_MASK: u32 = 0x7F00_0000;

    /// Undefined or unknown pixel format.
    pub const UNDEFINED: Self = Self(sys::PixelType_Gvsp_Undefined as u32);

    /// 8-bit monochrome pixels.
    pub const MONO8: Self = Self(sys::PixelType_Gvsp_Mono8 as u32);
    /// 10-bit monochrome pixels stored in 16-bit words.
    pub const MONO10: Self = Self(sys::PixelType_Gvsp_Mono10 as u32);
    /// Packed 10-bit monochrome pixels.
    pub const MONO10_PACKED: Self = Self(sys::PixelType_Gvsp_Mono10_Packed as u32);
    /// 12-bit monochrome pixels stored in 16-bit words.
    pub const MONO12: Self = Self(sys::PixelType_Gvsp_Mono12 as u32);
    /// Packed 12-bit monochrome pixels.
    pub const MONO12_PACKED: Self = Self(sys::PixelType_Gvsp_Mono12_Packed as u32);
    /// 14-bit monochrome pixels stored in 16-bit words.
    pub const MONO14: Self = Self(sys::PixelType_Gvsp_Mono14 as u32);
    /// 16-bit monochrome pixels.
    pub const MONO16: Self = Self(sys::PixelType_Gvsp_Mono16 as u32);

    /// 8-bit Bayer pixels in GR order.
    pub const BAYER_GR8: Self = Self(sys::PixelType_Gvsp_BayerGR8 as u32);
    /// 8-bit Bayer pixels in RG order.
    pub const BAYER_RG8: Self = Self(sys::PixelType_Gvsp_BayerRG8 as u32);
    /// 8-bit Bayer pixels in GB order.
    pub const BAYER_GB8: Self = Self(sys::PixelType_Gvsp_BayerGB8 as u32);
    /// 8-bit Bayer pixels in BG order.
    pub const BAYER_BG8: Self = Self(sys::PixelType_Gvsp_BayerBG8 as u32);

    /// Packed RGB with 8 bits per channel.
    pub const RGB8_PACKED: Self = Self(sys::PixelType_Gvsp_RGB8_Packed as u32);
    /// Packed BGR with 8 bits per channel.
    pub const BGR8_PACKED: Self = Self(sys::PixelType_Gvsp_BGR8_Packed as u32);
    /// Packed RGBA with 8 bits per channel.
    pub const RGBA8_PACKED: Self = Self(sys::PixelType_Gvsp_RGBA8_Packed as u32);
    /// Packed BGRA with 8 bits per channel.
    pub const BGRA8_PACKED: Self = Self(sys::PixelType_Gvsp_BGRA8_Packed as u32);

    /// Packed YUV 4:2:2 pixels.
    pub const YUV422_PACKED: Self = Self(sys::PixelType_Gvsp_YUV422_Packed as u32);
    /// Packed YUV 4:2:2 pixels in YUYV order.
    pub const YUV422_YUYV_PACKED: Self = Self(sys::PixelType_Gvsp_YUV422_YUYV_Packed as u32);

    /// Construct a pixel type from the SDK's raw wire-format code.
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the SDK's raw wire-format code.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Effective bits per pixel encoded in the format descriptor.
    #[inline]
    pub const fn bits_per_pixel(self) -> u32 {
        (self.0 >> 16) & 0xFF
    }

    /// Return whether the format descriptor identifies monochrome data.
    #[inline]
    pub const fn is_mono(self) -> bool {
        (self.0 & Self::CATEGORY_MASK) == 0x0100_0000
    }

    /// Return whether the format descriptor identifies color data.
    #[inline]
    pub const fn is_color(self) -> bool {
        (self.0 & Self::CATEGORY_MASK) == 0x0200_0000
    }

    /// Return whether the SDK's custom-format bit is set.
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

#[cfg(test)]
mod tests {
    use super::{Timeout, TransportLayer};

    // 有限等待不得退化成 SDK 的无限等待哨兵。
    #[test]
    fn finite_timeout_never_selects_infinite_wait() {
        assert_eq!(Timeout::Infinite.raw(), u32::MAX);
        assert_eq!(Timeout::Finite(0).raw(), 0);
        assert_eq!(Timeout::Finite(u32::MAX).raw(), u32::MAX - 1);
    }

    // transport 判定是位集语义，公开接口与 backend 共用同一实现。
    #[test]
    fn transport_predicates_cover_virtual_and_gentl_variants() {
        assert!(TransportLayer::VIR_GIGE.is_gige());
        assert!(TransportLayer::GENTL_GIGE.is_gige());
        assert!(!TransportLayer::USB.is_gige());
        assert!(TransportLayer::VIR_USB.is_usb());
    }
}
