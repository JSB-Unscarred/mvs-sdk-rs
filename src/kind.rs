//! SDK 枚举值与位集的 Rust 表示。

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::sys;

/// [`Sdk::open`](crate::Sdk::open) 使用的设备访问模式。
///
/// 模式与切换 key 只对原生 `GigE` 设备有意义，`GenTL` `GigE` 设备只接受独占、控制与监视三种；
/// USB3、Camera Link、`CoaXPress`、`XoF` 与虚拟设备忽略这两个参数，按控制权限打开。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum AccessMode {
    /// 独占权限。
    Exclusive = sys::MV_ACCESS_Exclusive,
    /// 可被其它应用以切换 key 抢占的独占权限。
    ExclusiveWithSwitch = sys::MV_ACCESS_ExclusiveWithSwitch,
    /// 控制权限。
    Control = sys::MV_ACCESS_Control,
    /// 可被其它应用以切换 key 抢占的控制权限。
    ControlWithSwitch = sys::MV_ACCESS_ControlWithSwitch,
    /// 以无 key 方式抢占控制权限。
    ControlSwitchEnable = sys::MV_ACCESS_ControlSwitchEnable,
    /// 以切换 key 抢占控制权限。
    ControlSwitchEnableWithKey = sys::MV_ACCESS_ControlSwitchEnableWithKey,
    /// 只读监视权限。
    Monitor = sys::MV_ACCESS_Monitor,
}

/// 枚举设备时使用的 transport 位集，可用 `|` 组合。
///
/// [`DeviceInfo::transport_layer`](crate::DeviceInfo::transport_layer) 返回单个 transport 值。
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TransportLayer(u32);

impl TransportLayer {
    /// `GigE` Vision 设备。
    pub const GIGE: Self = Self(sys::MV_GIGE_DEVICE);
    /// IEEE 1394 设备。
    pub const IEEE_1394: Self = Self(sys::MV_1394_DEVICE);
    /// USB3 Vision 设备。
    pub const USB: Self = Self(sys::MV_USB_DEVICE);
    /// Camera Link 设备。
    pub const CAMERALINK: Self = Self(sys::MV_CAMERALINK_DEVICE);
    /// 虚拟 `GigE` 设备。
    pub const VIR_GIGE: Self = Self(sys::MV_VIR_GIGE_DEVICE);
    /// 虚拟 USB 设备。
    pub const VIR_USB: Self = Self(sys::MV_VIR_USB_DEVICE);
    /// 经 `GenTL` 接入的 `GigE` 设备。
    pub const GENTL_GIGE: Self = Self(sys::MV_GENTL_GIGE_DEVICE);
    /// 经 `GenTL` 接入的 Camera Link 设备。
    pub const GENTL_CAMERALINK: Self = Self(sys::MV_GENTL_CAMERALINK_DEVICE);
    /// 经 `GenTL` 接入的 `CoaXPress` 设备。
    pub const GENTL_CXP: Self = Self(sys::MV_GENTL_CXP_DEVICE);
    /// 经 `GenTL` 接入的 `XoF` 设备。
    pub const GENTL_XOF: Self = Self(sys::MV_GENTL_XOF_DEVICE);
    /// 经 `GenTL` 接入的虚拟设备。
    pub const GENTL_VIR: Self = Self(sys::MV_GENTL_VIR_DEVICE);
    /// 当前 bindings 定义的全部 transport。
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

    /// 由 SDK 原始值构造。
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// 返回 SDK 原始值。
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// 是否包含 `other` 的全部位。
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for TransportLayer {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for TransportLayer {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Debug for TransportLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransportLayer(0x{:08X})", self.0)
    }
}

/// `GigE` Vision 像素格式码。
///
/// SDK 定义了上百种格式，这里只列出常用常量；其余值可用 [`PixelType::from_raw`] 表示。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelType(u32);

impl PixelType {
    /// 未定义的像素格式。
    pub const UNDEFINED: Self = Self(sys::PixelType_Gvsp_Undefined.cast_unsigned());
    /// 8 位单色。
    pub const MONO8: Self = Self(sys::PixelType_Gvsp_Mono8.cast_unsigned());
    /// 10 位单色，按 16 位存储。
    pub const MONO10: Self = Self(sys::PixelType_Gvsp_Mono10.cast_unsigned());
    /// 10 位单色，紧凑存储。
    pub const MONO10_PACKED: Self = Self(sys::PixelType_Gvsp_Mono10_Packed.cast_unsigned());
    /// 12 位单色，按 16 位存储。
    pub const MONO12: Self = Self(sys::PixelType_Gvsp_Mono12.cast_unsigned());
    /// 12 位单色，紧凑存储。
    pub const MONO12_PACKED: Self = Self(sys::PixelType_Gvsp_Mono12_Packed.cast_unsigned());
    /// 14 位单色，按 16 位存储。
    pub const MONO14: Self = Self(sys::PixelType_Gvsp_Mono14.cast_unsigned());
    /// 16 位单色。
    pub const MONO16: Self = Self(sys::PixelType_Gvsp_Mono16.cast_unsigned());
    /// 8 位 Bayer GR。
    pub const BAYER_GR8: Self = Self(sys::PixelType_Gvsp_BayerGR8.cast_unsigned());
    /// 8 位 Bayer RG。
    pub const BAYER_RG8: Self = Self(sys::PixelType_Gvsp_BayerRG8.cast_unsigned());
    /// 8 位 Bayer GB。
    pub const BAYER_GB8: Self = Self(sys::PixelType_Gvsp_BayerGB8.cast_unsigned());
    /// 8 位 Bayer BG。
    pub const BAYER_BG8: Self = Self(sys::PixelType_Gvsp_BayerBG8.cast_unsigned());
    /// 每通道 8 位的 RGB。
    pub const RGB8_PACKED: Self = Self(sys::PixelType_Gvsp_RGB8_Packed.cast_unsigned());
    /// 每通道 8 位的 BGR。
    pub const BGR8_PACKED: Self = Self(sys::PixelType_Gvsp_BGR8_Packed.cast_unsigned());
    /// 每通道 8 位的 RGBA。
    pub const RGBA8_PACKED: Self = Self(sys::PixelType_Gvsp_RGBA8_Packed.cast_unsigned());
    /// 每通道 8 位的 BGRA。
    pub const BGRA8_PACKED: Self = Self(sys::PixelType_Gvsp_BGRA8_Packed.cast_unsigned());
    /// YUV 4:2:2。
    pub const YUV422_PACKED: Self = Self(sys::PixelType_Gvsp_YUV422_Packed.cast_unsigned());
    /// YUV 4:2:2，YUYV 排列。
    pub const YUV422_YUYV_PACKED: Self =
        Self(sys::PixelType_Gvsp_YUV422_YUYV_Packed.cast_unsigned());

    /// 由 SDK 原始值构造。
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// 返回 SDK 原始值。
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// 格式码中编码的每像素有效位数。
    pub const fn bits_per_pixel(self) -> u32 {
        (self.0 >> 16) & 0xFF
    }

    /// 是否为单色格式。
    pub const fn is_mono(self) -> bool {
        self.0 & 0x7F00_0000 == 0x0100_0000
    }

    /// 是否为彩色格式。
    pub const fn is_color(self) -> bool {
        self.0 & 0x7F00_0000 == 0x0200_0000
    }
}

impl fmt::Debug for PixelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PixelType(0x{:08X})", self.0)
    }
}
