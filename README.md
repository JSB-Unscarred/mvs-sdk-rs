# mvs_sdk_rs

海康威视机器人 **MVS** 工业相机 SDK 的安全 Rust 封装。`mvs_sdk_rs` 将 MVS C SDK 的 FFI、错误码转换、buffer 生命周期和回调桥接封装在内部，对外提供设备枚举、相机控制、参数读写和图像采集 API。

当前仅支持 **Windows x86_64**。其它平台会暴露 stub API，便于跨平台工作区执行 `cargo check`，但不能实际连接相机。运行时需要 MVS SDK、`MVCAM_COMMON_RUNENV` 和 MVS DLL `PATH` 已正确配置。

## 快速开始

回调取流是最常用路径：初始化 SDK，枚举相机，打开设备，设置参数，注册图像回调，然后开始采集。

```rust
use mvs_sdk_rs::{MvsResult, Sdk, TransportLayer};

fn main() -> MvsResult<()> {
    let sdk = Sdk::init()?;
    println!("MVS SDK version: 0x{:08X}", sdk.sdk_version());

    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let Some(device) = devices.iter().next() else {
        println!("No camera found");
        return Ok(());
    };

    println!(
        "Open camera: {} {} SN={}",
        device.manufacturer(),
        device.model(),
        device.serial()
    );

    let mut cam = device.open_exclusive()?;
    cam.set_enum("TriggerMode", "Off")?;
    cam.set_float("ExposureTime", 5000.0)?;

    cam.register_image_callback(|frame| {
        let info = frame.info();
        println!(
            "frame={} size={}x{} bytes={}",
            info.frame_num(),
            info.width(),
            info.height(),
            frame.data().len()
        );
    })?;

    cam.start_grabbing()?;
    std::thread::sleep(std::time::Duration::from_secs(3));
    cam.stop_grabbing()?;

    Ok(())
}
```

## 轮询取图

需要主动等待单帧时，使用 `get_image_buffer`。返回的 `FrameGuard` 在 drop 时自动释放 SDK buffer。

```rust
cam.start_grabbing()?;

let guard = cam.get_image_buffer(1000)?;
let frame = guard.frame();
println!("{:?}", frame.info());

let _owned = frame.to_owned();
guard.release()?;
```

## API 参考

以下覆盖 Windows SDK 构建下 crate 对外暴露的安全 API。`src/bindings.rs` 中的原始 MVS FFI 由 crate 内部使用，普通业务代码通常不需要直接接触。

### 导出路径

| 路径 | 导出 |
| --- | --- |
| `mvs_sdk_rs::*` | `Sdk`、`TransportLayer`、`DeviceList`、`DeviceIter`、`DeviceInfo`、`AccessMode`、`Camera`、`EventInfo`、`Frame`、`FrameGuard`、`FrameInfo`、`OwnedFrame`、`PixelType`、`MvsResult`、`MvsError` |
| `mvs_sdk_rs::error::*` | `MvsResult`、`MvsError` |
| `Camera` 方法返回值 | `IntNode`、`FloatNode`、`EnumNode`，分别由 `get_int_range`、`get_float_range`、`get_enum_info` 返回 |

### `Sdk`

| API | 签名 | 说明 |
| --- | --- | --- |
| `Sdk::init` | `fn init() -> MvsResult<Arc<Sdk>>` | 初始化 MVS SDK。进程内只初始化一次，多次调用成本很低。 |
| `Sdk::sdk_version` | `fn sdk_version(&self) -> u32` | 返回 MVS SDK packed version，解释方式以 MVS 文档为准。 |
| `Sdk::enumerate_devices` | `fn enumerate_devices(self: &Arc<Self>, layers: TransportLayer) -> MvsResult<DeviceList>` | 枚举指定传输层上的设备，常用 <code>TransportLayer::GIGE &#124; TransportLayer::USB</code>。 |

### `TransportLayer`

| API | 签名 / 取值 | 说明 |
| --- | --- | --- |
| `UNKNOWN` | `TransportLayer` | 未知设备类型。 |
| `GIGE` | `TransportLayer` | GigE Vision 设备。 |
| `USB` | `TransportLayer` | USB3 Vision 设备。 |
| `CAMERALINK` | `TransportLayer` | Camera Link 设备。 |
| `VIR_GIGE` | `TransportLayer` | 虚拟 GigE 设备。 |
| `VIR_USB` | `TransportLayer` | 虚拟 USB 设备。 |
| `GENTL_GIGE` | `TransportLayer` | GenTL GigE 设备。 |
| `GENTL_CAMERALINK` | `TransportLayer` | GenTL Camera Link 设备。 |
| `GENTL_CXP` | `TransportLayer` | GenTL CoaXPress 设备。 |
| `GENTL_XOF` | `TransportLayer` | GenTL XoF 设备。 |
| `GENTL_VIR` | `TransportLayer` | GenTL 虚拟设备。 |
| `ALL` | `TransportLayer` | 枚举 SDK 已知的所有设备类型。 |
| `from_raw` | `const fn from_raw(raw: u32) -> TransportLayer` | 从 MVS 原始 bitmask 构造。 |
| `raw` | `const fn raw(self) -> u32` | 返回 MVS 原始 bitmask。 |
| `contains` | `const fn contains(self, other: TransportLayer) -> bool` | 判断当前 bitset 是否包含另一组传输层。 |
| `BitOr` / `BitOrAssign` | <code>a &#124; b</code>、<code>a &#124;= b</code> | 组合多个传输层。 |

### `DeviceList` / `DeviceIter`

| API | 签名 | 说明 |
| --- | --- | --- |
| `DeviceList::len` | `fn len(&self) -> usize` | 返回设备数量。 |
| `DeviceList::is_empty` | `fn is_empty(&self) -> bool` | 判断是否没有设备。 |
| `DeviceList::iter` | `fn iter(&self) -> DeviceIter<'_>` | 遍历设备列表。 |
| `DeviceList::get` | `fn get(&self, index: usize) -> Option<DeviceInfo<'_>>` | 按索引获取设备信息。 |
| `DeviceIter` | `Iterator<Item = DeviceInfo<'_>>` | 设备迭代器，同时实现 `ExactSizeIterator`。 |

### `DeviceInfo`

| API | 签名 | 说明 |
| --- | --- | --- |
| `transport_layer` | `fn transport_layer(&self) -> TransportLayer` | 返回设备所属传输层。 |
| `is_gige` | `fn is_gige(&self) -> bool` | 判断是否为 GigE / virtual GigE / GenTL GigE。 |
| `is_usb` | `fn is_usb(&self) -> bool` | 判断是否为 USB / virtual USB。 |
| `manufacturer` | `fn manufacturer(&self) -> String` | 读取厂商名称。 |
| `model` | `fn model(&self) -> String` | 读取型号名称。 |
| `serial` | `fn serial(&self) -> String` | 读取序列号。 |
| `user_defined_name` | `fn user_defined_name(&self) -> String` | 读取用户自定义名称。 |
| `ip` | `fn ip(&self) -> Option<Ipv4Addr>` | 读取 GigE 设备当前 IP，其它传输层返回 `None`。 |
| `host_nic_ip` | `fn host_nic_ip(&self) -> Option<Ipv4Addr>` | 读取 GigE 设备所在主机网卡 IP，其它传输层返回 `None`。 |
| `is_accessible` | `fn is_accessible(&self, mode: AccessMode) -> bool` | 检查设备当前是否能用指定模式打开。 |
| `open` | `fn open(&self, mode: AccessMode) -> MvsResult<Camera>` | 按指定访问模式打开相机。 |
| `open_exclusive` | `fn open_exclusive(&self) -> MvsResult<Camera>` | 以 `AccessMode::Exclusive` 打开。 |
| `open_control` | `fn open_control(&self) -> MvsResult<Camera>` | 以 `AccessMode::Control` 打开。 |
| `as_raw` | `fn as_raw(&self) -> *const MV_CC_DEVICE_INFO` | 返回底层 SDK 设备信息指针，适合高级用法。 |

### `AccessMode`

| 变体 | 说明 |
| --- | --- |
| `Exclusive` | 独占访问。 |
| `ExclusiveWithSwitch` | 独占访问，并允许控制权切换。 |
| `Control` | 控制访问。 |
| `ControlWithSwitch` | 控制访问，并允许控制权切换。 |
| `ControlSwitchEnable` | 启用控制权切换。 |
| `ControlSwitchEnableWithKey` | 通过 key 启用控制权切换。 |
| `Monitor` | 监视模式。 |

### `Camera`

| API | 签名 | 说明 |
| --- | --- | --- |
| `as_raw_handle` | `fn as_raw_handle(&self) -> *mut c_void` | 返回底层 SDK handle，适合高级用法。 |
| `is_connected` | `fn is_connected(&self) -> bool` | 检查设备是否仍连接。 |
| `start_grabbing` | `fn start_grabbing(&mut self) -> MvsResult<()>` | 开始采集。 |
| `stop_grabbing` | `fn stop_grabbing(&mut self) -> MvsResult<()>` | 停止采集。 |
| `get_image_buffer` | `fn get_image_buffer(&mut self, timeout_ms: u32) -> MvsResult<FrameGuard<'_>>` | 轮询获取一帧图像，超时时间单位为毫秒。 |
| `register_image_callback` | `fn register_image_callback<F>(&mut self, f: F) -> MvsResult<()> where F: FnMut(&Frame<'_>) + Send + 'static` | 注册图像回调。回调在 SDK 采集线程中执行，建议尽量短。 |
| `unregister_image_callback` | `fn unregister_image_callback(&mut self) -> MvsResult<()>` | 注销图像回调。 |
| `register_exception_callback` | `fn register_exception_callback<F>(&mut self, f: F) -> MvsResult<()> where F: FnMut(u32) + Send + 'static` | 注册异常回调，参数为 SDK 原始消息类型。 |
| `register_event_callback` | `fn register_event_callback<F>(&mut self, event_name: &str, f: F) -> MvsResult<()> where F: FnMut(&EventInfo<'_>) + Send + 'static` | 注册指定 GenICam 事件回调。 |
| `event_notification_on` | `fn event_notification_on(&self, event_name: &str) -> MvsResult<()>` | 开启指定事件通知。 |
| `event_notification_off` | `fn event_notification_off(&self, event_name: &str) -> MvsResult<()>` | 关闭指定事件通知。 |
| `set_int` | `fn set_int(&self, key: &str, value: i64) -> MvsResult<()>` | 设置整数节点，例如 `Width`、`Height`、`OffsetX`。 |
| `get_int` | `fn get_int(&self, key: &str) -> MvsResult<i64>` | 读取整数节点当前值。 |
| `get_int_range` | `fn get_int_range(&self, key: &str) -> MvsResult<IntNode>` | 读取整数节点当前值和范围。 |
| `set_float` | `fn set_float(&self, key: &str, value: f32) -> MvsResult<()>` | 设置浮点节点，例如 `ExposureTime`、`Gain`。 |
| `get_float` | `fn get_float(&self, key: &str) -> MvsResult<f32>` | 读取浮点节点当前值。 |
| `get_float_range` | `fn get_float_range(&self, key: &str) -> MvsResult<FloatNode>` | 读取浮点节点当前值和范围。 |
| `set_bool` | `fn set_bool(&self, key: &str, value: bool) -> MvsResult<()>` | 设置布尔节点，例如 `ReverseX`。 |
| `get_bool` | `fn get_bool(&self, key: &str) -> MvsResult<bool>` | 读取布尔节点。 |
| `set_enum` | `fn set_enum(&self, key: &str, value: &str) -> MvsResult<()>` | 通过符号名设置枚举节点，例如 `set_enum("TriggerMode", "Off")`。 |
| `set_enum_value` | `fn set_enum_value(&self, key: &str, value: u32) -> MvsResult<()>` | 通过数值设置枚举节点。 |
| `get_enum` | `fn get_enum(&self, key: &str) -> MvsResult<u32>` | 读取枚举节点当前数值。 |
| `get_enum_info` | `fn get_enum_info(&self, key: &str) -> MvsResult<EnumNode>` | 读取枚举节点当前数值和支持值列表。 |
| `set_string` | `fn set_string(&self, key: &str, value: &str) -> MvsResult<()>` | 设置字符串节点，例如 `DeviceUserID`。 |
| `get_string` | `fn get_string(&self, key: &str) -> MvsResult<String>` | 读取字符串节点。 |
| `exec_command` | `fn exec_command(&self, key: &str) -> MvsResult<()>` | 执行命令节点，例如 `TriggerSoftware`。 |

`Camera` 在 drop 时会依次停止采集、注销回调、关闭设备并销毁 handle。`Camera` 实现 `Send`，但不实现 `Sync`，同一相机实例的并发访问需要外部同步。

### 参数节点信息

| 类型 | 字段 | 说明 |
| --- | --- | --- |
| `IntNode` | `current: i64` | 当前整数值。 |
| `IntNode` | `min: i64` | 最小值。 |
| `IntNode` | `max: i64` | 最大值。 |
| `IntNode` | `inc: i64` | 步进值。 |
| `FloatNode` | `current: f32` | 当前浮点值。 |
| `FloatNode` | `min: f32` | 最小值。 |
| `FloatNode` | `max: f32` | 最大值。 |
| `EnumNode` | `current: u32` | 当前枚举数值。 |
| `EnumNode` | `supported: Vec<u32>` | 支持的枚举数值列表。 |

### `FrameInfo`

| API | 签名 | 说明 |
| --- | --- | --- |
| `width` | `fn width(&self) -> u32` | 图像宽度。 |
| `height` | `fn height(&self) -> u32` | 图像高度。 |
| `pixel_type` | `fn pixel_type(&self) -> PixelType` | 像素格式。 |
| `frame_num` | `fn frame_num(&self) -> u32` | 帧号。 |
| `frame_len` | `fn frame_len(&self) -> u32` | 图像数据长度。 |
| `offset_x` | `fn offset_x(&self) -> u32` | X 偏移。 |
| `offset_y` | `fn offset_y(&self) -> u32` | Y 偏移。 |
| `gain` | `fn gain(&self) -> f32` | 帧元数据中的增益。 |
| `exposure_time` | `fn exposure_time(&self) -> f32` | 帧元数据中的曝光时间。 |
| `trigger_index` | `fn trigger_index(&self) -> u32` | 触发序号。 |
| `lost_packets` | `fn lost_packets(&self) -> u32` | 丢包数量。 |
| `device_timestamp` | `fn device_timestamp(&self) -> u64` | 设备时间戳。 |
| `host_timestamp_raw` | `fn host_timestamp_raw(&self) -> i64` | SDK 返回的主机原始时间戳。 |
| `host_timestamp` | `fn host_timestamp(&self) -> Duration` | 将主机时间戳按 100 ns tick 转为 `Duration`。 |

### `Frame` / `FrameGuard` / `OwnedFrame`

| 类型 | API | 签名 | 说明 |
| --- | --- | --- | --- |
| `Frame<'a>` | `data` | `fn data(&self) -> &[u8]` | 借用当前帧像素数据。 |
| `Frame<'a>` | `info` | `fn info(&self) -> &FrameInfo<'a>` | 读取当前帧元数据。 |
| `Frame<'a>` | `to_owned` | `fn to_owned(&self) -> OwnedFrame` | 复制为拥有独立 buffer 的帧。 |
| `FrameGuard<'cam>` | `frame` | `fn frame(&self) -> Frame<'_>` | 从 SDK buffer 借用一帧。 |
| `FrameGuard<'cam>` | `info` | `fn info(&self) -> FrameInfo<'_>` | 直接读取 guard 内的帧元数据。 |
| `FrameGuard<'cam>` | `to_owned` | `fn to_owned(&self) -> OwnedFrame` | 复制为 `OwnedFrame`。 |
| `FrameGuard<'cam>` | `release` | `fn release(self) -> MvsResult<()>` | 显式释放 SDK buffer；不调用也会在 drop 时自动释放。 |
| `OwnedFrame` | `data` | `pub data: Vec<u8>` | 拥有的像素数据。 |
| `OwnedFrame` | `info` | `fn info(&self) -> FrameInfo<'_>` | 读取拥有帧的元数据。 |
| `OwnedFrame` | `as_frame` | `fn as_frame(&self) -> Frame<'_>` | 将拥有帧临时借用为 `Frame`。 |

### `PixelType`

| API | 签名 / 取值 | 说明 |
| --- | --- | --- |
| `UNDEFINED` | `PixelType` | 未定义像素格式。 |
| `MONO8` | `PixelType` | 8-bit Mono。 |
| `MONO10` | `PixelType` | 10-bit Mono。 |
| `MONO10_PACKED` | `PixelType` | packed 10-bit Mono。 |
| `MONO12` | `PixelType` | 12-bit Mono。 |
| `MONO12_PACKED` | `PixelType` | packed 12-bit Mono。 |
| `MONO14` | `PixelType` | 14-bit Mono。 |
| `MONO16` | `PixelType` | 16-bit Mono。 |
| `BAYER_GR8` | `PixelType` | Bayer GR 8-bit。 |
| `BAYER_RG8` | `PixelType` | Bayer RG 8-bit。 |
| `BAYER_GB8` | `PixelType` | Bayer GB 8-bit。 |
| `BAYER_BG8` | `PixelType` | Bayer BG 8-bit。 |
| `RGB8_PACKED` | `PixelType` | packed RGB 8-bit。 |
| `BGR8_PACKED` | `PixelType` | packed BGR 8-bit。 |
| `RGBA8_PACKED` | `PixelType` | packed RGBA 8-bit。 |
| `BGRA8_PACKED` | `PixelType` | packed BGRA 8-bit。 |
| `YUV422_PACKED` | `PixelType` | packed YUV422。 |
| `YUV422_YUYV_PACKED` | `PixelType` | packed YUV422 YUYV。 |
| `from_raw` | `const fn from_raw(raw: u32) -> PixelType` | 从 MVS 原始像素格式值构造。 |
| `raw` | `const fn raw(self) -> u32` | 返回 MVS 原始像素格式值。 |
| `bits_per_pixel` | `const fn bits_per_pixel(self) -> u32` | 读取格式描述中的有效 bit 数。 |
| `is_mono` | `const fn is_mono(self) -> bool` | 判断是否为 Mono 格式。 |
| `is_color` | `const fn is_color(self) -> bool` | 判断是否为彩色格式。 |
| `is_custom` | `const fn is_custom(self) -> bool` | 判断是否为自定义格式。 |

### `EventInfo`

| API | 签名 | 说明 |
| --- | --- | --- |
| `name` | `fn name(&self) -> Cow<'_, str>` | 事件名称。 |
| `event_id` | `fn event_id(&self) -> u16` | 事件 ID。 |
| `stream_channel` | `fn stream_channel(&self) -> u16` | 流通道。 |
| `block_id` | `fn block_id(&self) -> u64` | 事件 block ID。 |
| `timestamp` | `fn timestamp(&self) -> u64` | 事件时间戳。 |

### `MvsResult` / `MvsError`

| API | 签名 / 取值 | 说明 |
| --- | --- | --- |
| `MvsResult<T>` | `type MvsResult<T> = Result<T, MvsError>` | crate 统一返回类型。 |
| `MvsError::raw_code` | `fn raw_code(&self) -> Option<u32>` | 返回原始 SDK 错误码；Rust 侧错误返回 `None`。 |
| `From<c_int>` | `impl From<c_int> for MvsError` | 将 SDK `c_int` 返回码转为 `MvsError`。 |
| `From<u32>` | `impl From<u32> for MvsError` | 将 SDK `u32` 错误码转为 `MvsError`。 |

#### `MvsError` 变体

| 变体 | 说明 |
| --- | --- |
| `Handle` | 无效 handle。 |
| `NotSupported` | 操作不支持。 |
| `BufferOverflow` | buffer 溢出。 |
| `CallOrder` | 调用顺序错误。 |
| `Parameter` | 参数无效。 |
| `Resource` | 资源分配失败。 |
| `NoData` | 没有数据。 |
| `Precondition` | 前置条件失败或环境变化。 |
| `Version` | 版本不匹配。 |
| `NotEnoughBuffer` | buffer 不足。 |
| `AbnormalImage` | 图像异常，可能由丢包导致。 |
| `LoadLibrary` | 加载库失败。 |
| `NoOutputBuffer` | 没有可用输出 buffer。 |
| `Encrypt` | 加密错误。 |
| `OpenFile` | 打开文件失败。 |
| `BufferInUse` | buffer 正在使用。 |
| `BufferInvalid` | buffer 地址无效。 |
| `NoAlignBuffer` | buffer 对齐错误。 |
| `NotEnoughBufferNum` | buffer 数量不足。 |
| `PortInUse` | 端口被占用。 |
| `ImageDecodec` | 图像解码错误。 |
| `Uint32Limit` | 图像大小超过 `u32` 限制。 |
| `ImageHeight` | 图像高度异常。 |
| `NotEnoughDdr` | DDR 缓存不足。 |
| `NotEnoughStream` | 流通道不足。 |
| `NoResponse` | 设备无响应。 |
| `UnknownGeneric` | 未知通用错误。 |
| `GcGeneric` | GenICam 通用错误。 |
| `GcArgument` | GenICam 参数错误。 |
| `GcRange` | GenICam 值超出范围。 |
| `GcProperty` | GenICam 属性错误。 |
| `GcRuntime` | GenICam 运行时错误。 |
| `GcLogical` | GenICam 逻辑错误。 |
| `GcAccess` | GenICam 节点访问条件错误。 |
| `GcTimeout` | GenICam 超时。 |
| `GcDynamicCast` | GenICam dynamic cast 错误。 |
| `GcUnknown` | GenICam 未知错误。 |
| `NotImplemented` | GigE 设备未实现命令。 |
| `InvalidAddress` | GigE 地址无效。 |
| `WriteProtect` | GigE 写保护。 |
| `AccessDenied` | GigE 访问被拒绝。 |
| `Busy` | GigE 设备忙或网络断开。 |
| `Packet` | GigE 网络包错误。 |
| `Net` | GigE 网络错误。 |
| `IpConflict` | GigE 设备 IP 冲突。 |
| `UsbRead` | USB 读错误。 |
| `UsbWrite` | USB 写错误。 |
| `UsbDevice` | USB 设备异常。 |
| `UsbGenicam` | USB GenICam 错误。 |
| `UsbBandwidth` | USB 带宽不足。 |
| `UsbDriver` | USB 驱动不匹配或缺失。 |
| `UsbUnknown` | USB 未知错误。 |
| `UpgFileMismatch` | 固件升级文件不匹配。 |
| `UpgLanguageMismatch` | 固件升级语言不匹配。 |
| `UpgConflict` | 固件升级冲突，通常表示正在升级。 |
| `UpgInnerErr` | 固件升级设备内部错误。 |
| `UpgUnknown` | 固件升级未知错误。 |
| `Unknown(u32)` | 未识别的 SDK 错误码，保留原始值。 |
| `Nul(NulError)` | Rust 字符串包含内部 NUL 字节。 |
| `Utf8(Utf8Error)` | SDK 返回的数据不是合法 UTF-8。 |

### Trait 和行为

| 类型 | Trait / 行为 |
| --- | --- |
| `TransportLayer` | `Copy`、`Clone`、`PartialEq`、`Eq`、`Default`、`Debug`、`BitOr`、`BitOrAssign` |
| `DeviceList` | `Debug`、`Send`、`Sync` |
| `DeviceIter` | `Iterator<Item = DeviceInfo<'_>>`、`ExactSizeIterator` |
| `DeviceInfo` | `Copy`、`Clone`、`Debug` |
| `AccessMode` | `Copy`、`Clone`、`PartialEq`、`Eq`、`Debug` |
| `Camera` | `Debug`、`Drop`、`Send`；不实现 `Sync` |
| `IntNode` | `Copy`、`Clone`、`Debug` |
| `FloatNode` | `Copy`、`Clone`、`Debug` |
| `EnumNode` | `Clone`、`Debug` |
| `EventInfo` | `Copy`、`Clone`、`Debug` |
| `PixelType` | `Copy`、`Clone`、`PartialEq`、`Eq`、`Hash`、`Debug` |
| `FrameInfo` | `Copy`、`Clone`、`Debug` |
| `Frame` | `Debug` |
| `OwnedFrame` | `Clone`、`Debug` |
| `FrameGuard` | `Drop` 时自动释放 SDK buffer |
| `MvsError` | `Debug`、`Display`、`std::error::Error` |

### 非 Windows stub

非 Windows 平台只用于 `cargo check`。stub 保留同名类型和方法表面，但不链接真实 MVS SDK：`Sdk::init` 返回 `MvsError::Stub(String)`，其它运行时方法大多是 `unimplemented!()`。

stub 下的 `MvsError` 变体为：`Handle`、`NotSupported`、`NoData`、`Parameter`、`Resource`、`Unknown(u32)`、`Stub(String)`。

## 维护：重新生成 bindings

`src/bindings.rs` 已提交到仓库，普通使用不需要安装 libclang。升级 MVS SDK 后，可通过 `bindgen` feature 重新生成：

```cmd
cargo build --features bindgen
```

执行前请安装 LLVM/libclang，并确保 `MVCAM_COMMON_RUNENV` 指向包含 `Includes/MvCameraControl.h` 的 MVS SDK 开发目录。

## License

MIT
