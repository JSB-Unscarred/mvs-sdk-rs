//! Platform-independent public value types.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

/// Device access mode passed to [`DeviceInfo::open`](crate::DeviceInfo::open).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AccessMode {
    Exclusive,
    ExclusiveWithSwitch,
    Control,
    ControlWithSwitch,
    ControlSwitchEnable,
    ControlSwitchEnableWithKey,
    Monitor,
}

/// Full integer-node information: current value plus its allowed range.
#[derive(Copy, Clone, Debug)]
pub struct IntNode {
    pub current: i64,
    pub min: i64,
    pub max: i64,
    pub inc: i64,
}

/// Full float-node information: current value plus min/max.
#[derive(Copy, Clone, Debug)]
pub struct FloatNode {
    pub current: f32,
    pub min: f32,
    pub max: f32,
}

/// Enum-node information: current numeric value and the list of allowed values.
#[derive(Clone, Debug)]
pub struct EnumNode {
    pub current: u32,
    pub supported: Vec<u32>,
}

/// Bit set of transport-layer protocols to enumerate. Combine with `|`.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct TransportLayer(u32);

impl TransportLayer {
    pub const UNKNOWN: Self = Self(0);
    pub const GIGE: Self = Self(1);
    pub const USB: Self = Self(4);
    pub const CAMERALINK: Self = Self(8);
    pub const VIR_GIGE: Self = Self(0x10);
    pub const VIR_USB: Self = Self(0x20);
    pub const GENTL_GIGE: Self = Self(0x40);
    pub const GENTL_CAMERALINK: Self = Self(0x80);
    pub const GENTL_CXP: Self = Self(0x100);
    pub const GENTL_XOF: Self = Self(0x200);
    pub const GENTL_VIR: Self = Self(0x800);

    /// Enumerate every type the SDK knows about.
    pub const ALL: Self = Self(0xFFFF_FFFF);

    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

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
    pub const UNDEFINED: Self = Self(0xFFFF_FFFF);

    pub const MONO8: Self = Self(0x0108_0001);
    pub const MONO10: Self = Self(0x0110_0003);
    pub const MONO10_PACKED: Self = Self(0x010C_0004);
    pub const MONO12: Self = Self(0x0110_0005);
    pub const MONO12_PACKED: Self = Self(0x010C_0006);
    pub const MONO14: Self = Self(0x0110_0025);
    pub const MONO16: Self = Self(0x0110_0007);

    pub const BAYER_GR8: Self = Self(0x0108_0008);
    pub const BAYER_RG8: Self = Self(0x0108_0009);
    pub const BAYER_GB8: Self = Self(0x0108_000A);
    pub const BAYER_BG8: Self = Self(0x0108_000B);

    pub const RGB8_PACKED: Self = Self(0x0218_0014);
    pub const BGR8_PACKED: Self = Self(0x0218_0015);
    pub const RGBA8_PACKED: Self = Self(0x0220_0016);
    pub const BGRA8_PACKED: Self = Self(0x0220_0017);

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

    /// Effective bits per pixel encoded in the format descriptor.
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

#[cfg(all(test, target_os = "windows", target_arch = "x86_64"))]
mod tests {
    use super::TransportLayer;
    use crate::sys;

    #[test]
    fn transport_layer_values_match_the_native_sdk() {
        assert_eq!(TransportLayer::UNKNOWN.raw(), sys::MV_UNKNOW_DEVICE);
        assert_eq!(TransportLayer::GIGE.raw(), sys::MV_GIGE_DEVICE);
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
    }
}
