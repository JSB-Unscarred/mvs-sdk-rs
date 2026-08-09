//! Platform-independent public value types.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

/// Device access mode passed to [`DeviceInfo::open`](crate::DeviceInfo::open).
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
    /// Enable control-owner switching with the supplied GigE switchover key.
    ControlSwitchEnableWithKey(u16),
    /// Read-only monitoring access.
    Monitor,
}

/// Full integer-node information: current value plus its allowed range.
#[derive(Copy, Clone, Debug)]
pub struct IntNode {
    /// Current node value.
    pub current: i64,
    /// Smallest accepted value.
    pub min: i64,
    /// Largest accepted value.
    pub max: i64,
    /// Required increment between accepted values.
    pub inc: i64,
}

/// Full float-node information: current value plus min/max.
#[derive(Copy, Clone, Debug)]
pub struct FloatNode {
    /// Current node value.
    pub current: f32,
    /// Smallest accepted value.
    pub min: f32,
    /// Largest accepted value.
    pub max: f32,
}

/// Enum-node information: current numeric value and the list of allowed values.
#[derive(Clone, Debug)]
pub struct EnumNode {
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
    pub const UNKNOWN: Self = Self(0);
    /// GigE Vision devices.
    pub const GIGE: Self = Self(1);
    /// IEEE 1394-a/b devices.
    pub const IEEE_1394: Self = Self(2);
    /// USB3 Vision devices.
    pub const USB: Self = Self(4);
    /// Camera Link devices.
    pub const CAMERALINK: Self = Self(8);
    /// Virtual GigE devices.
    pub const VIR_GIGE: Self = Self(0x10);
    /// Virtual USB devices.
    pub const VIR_USB: Self = Self(0x20);
    /// GigE devices exposed through GenTL.
    pub const GENTL_GIGE: Self = Self(0x40);
    /// Camera Link devices exposed through GenTL.
    pub const GENTL_CAMERALINK: Self = Self(0x80);
    /// CoaXPress devices exposed through GenTL.
    pub const GENTL_CXP: Self = Self(0x100);
    /// XoF devices exposed through GenTL.
    pub const GENTL_XOF: Self = Self(0x200);
    /// Virtual devices exposed through GenTL.
    pub const GENTL_VIR: Self = Self(0x800);

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
    pub const UNDEFINED: Self = Self(0xFFFF_FFFF);

    /// 8-bit monochrome pixels.
    pub const MONO8: Self = Self(0x0108_0001);
    /// 10-bit monochrome pixels stored in 16-bit words.
    pub const MONO10: Self = Self(0x0110_0003);
    /// Packed 10-bit monochrome pixels.
    pub const MONO10_PACKED: Self = Self(0x010C_0004);
    /// 12-bit monochrome pixels stored in 16-bit words.
    pub const MONO12: Self = Self(0x0110_0005);
    /// Packed 12-bit monochrome pixels.
    pub const MONO12_PACKED: Self = Self(0x010C_0006);
    /// 14-bit monochrome pixels stored in 16-bit words.
    pub const MONO14: Self = Self(0x0110_0025);
    /// 16-bit monochrome pixels.
    pub const MONO16: Self = Self(0x0110_0007);

    /// 8-bit Bayer pixels in GR order.
    pub const BAYER_GR8: Self = Self(0x0108_0008);
    /// 8-bit Bayer pixels in RG order.
    pub const BAYER_RG8: Self = Self(0x0108_0009);
    /// 8-bit Bayer pixels in GB order.
    pub const BAYER_GB8: Self = Self(0x0108_000A);
    /// 8-bit Bayer pixels in BG order.
    pub const BAYER_BG8: Self = Self(0x0108_000B);

    /// Packed RGB with 8 bits per channel.
    pub const RGB8_PACKED: Self = Self(0x0218_0014);
    /// Packed BGR with 8 bits per channel.
    pub const BGR8_PACKED: Self = Self(0x0218_0015);
    /// Packed RGBA with 8 bits per channel.
    pub const RGBA8_PACKED: Self = Self(0x0220_0016);
    /// Packed BGRA with 8 bits per channel.
    pub const BGRA8_PACKED: Self = Self(0x0220_0017);

    /// Packed YUV 4:2:2 pixels.
    pub const YUV422_PACKED: Self = Self(0x0210_001F);
    /// Packed YUV 4:2:2 pixels in YUYV order.
    pub const YUV422_YUYV_PACKED: Self = Self(0x0210_0032);

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
    use super::{PixelType, TransportLayer};
    use crate::sys;

    // 验证公开 bit/code 类型与 native SDK 保持 ABI 对应。
    #[test]
    fn public_codes_match_the_native_sdk() {
        assert_eq!(TransportLayer::UNKNOWN.raw(), sys::MV_UNKNOW_DEVICE);
        assert_eq!(TransportLayer::GIGE.raw(), sys::MV_GIGE_DEVICE);
        assert_eq!(TransportLayer::IEEE_1394.raw(), sys::MV_1394_DEVICE);
        assert_eq!(TransportLayer::USB.raw(), sys::MV_USB_DEVICE);
        assert_eq!(TransportLayer::CAMERALINK.raw(), sys::MV_CAMERALINK_DEVICE);
        assert_eq!(TransportLayer::VIR_GIGE.raw(), sys::MV_VIR_GIGE_DEVICE);
        assert_eq!(TransportLayer::VIR_USB.raw(), sys::MV_VIR_USB_DEVICE);
        assert_eq!(TransportLayer::GENTL_GIGE.raw(), sys::MV_GENTL_GIGE_DEVICE);
        assert_eq!(
            TransportLayer::GENTL_CAMERALINK.raw(),
            sys::MV_GENTL_CAMERALINK_DEVICE
        );
        assert_eq!(TransportLayer::GENTL_CXP.raw(), sys::MV_GENTL_CXP_DEVICE);
        assert_eq!(TransportLayer::GENTL_XOF.raw(), sys::MV_GENTL_XOF_DEVICE);
        assert_eq!(TransportLayer::GENTL_VIR.raw(), sys::MV_GENTL_VIR_DEVICE);
        assert_eq!(
            TransportLayer::ALL.raw(),
            sys::MV_GIGE_DEVICE
                | sys::MV_1394_DEVICE
                | sys::MV_USB_DEVICE
                | sys::MV_CAMERALINK_DEVICE
                | sys::MV_VIR_GIGE_DEVICE
                | sys::MV_VIR_USB_DEVICE
                | sys::MV_GENTL_GIGE_DEVICE
                | sys::MV_GENTL_CAMERALINK_DEVICE
                | sys::MV_GENTL_CXP_DEVICE
                | sys::MV_GENTL_XOF_DEVICE
                | sys::MV_GENTL_VIR_DEVICE
        );

        let mono = PixelType::from_raw(sys::PixelType_Gvsp_HB_Mono8 as u32);
        assert!(mono.is_mono());
        assert!(!mono.is_color());
        assert!(mono.is_custom());

        let color = PixelType::from_raw(sys::PixelType_Gvsp_HB_RGB8_Packed as u32);
        assert!(!color.is_mono());
        assert!(color.is_color());
        assert!(color.is_custom());

        assert!(PixelType::MONO8.is_mono());
        assert!(PixelType::RGB8_PACKED.is_color());
        assert!(!PixelType::UNDEFINED.is_mono());
        assert!(!PixelType::UNDEFINED.is_color());
    }
}
