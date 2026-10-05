# mvs-sdk

[English](README.en.md)

海康机器人（Hikrobot）MVS 工业相机 SDK 的安全 Rust 封装（非官方）。原始 FFI 在 `mvs-sdk-sys` 中，
也可以经 `mvs_sdk::sys` 访问。

## 环境

- 只支持 `x86_64-pc-windows-msvc`。
- 编译不需要安装 MVS。
- 运行时需要安装 MVS 4.7.0 或更新版本，并且 `MvCameraControl.dll` 所在目录在 `PATH` 中（MVS 安装程序默认会添加）。
  找不到该 DLL 时，程序在进入 `main` 之前就以 `0xC0000135` 退出，不输出任何信息。

```toml
[dependencies]
mvs-sdk = "0.1"
```

## 示例

```rust,no_run
use std::time::Duration;

use mvs_sdk::{AccessMode, Sdk, TransportLayer};

fn main() -> mvs_sdk::Result<()> {
    let sdk = Sdk::new()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let mut camera = sdk.open(&devices[0], AccessMode::Exclusive, 0)?;
    camera.set_float(c"ExposureTime", 10_000.0)?;

    let grabbing = camera.start_grabbing()?;
    let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
    let frame = buffer.frame();
    println!("{:?}, {} bytes", frame.info, frame.data.len());
    Ok(())
}
```

[`examples/`](examples) 中有完整示例：枚举设备（`enumerate_devices`）、主动取图（`grab_pull`）与
callback 取图（`grab_callback`）。

## 使用须知

- **会话**：一个进程只有一个 SDK 会话，`Sdk::new` 与 `clone` 得到的都是它。`Sdk` 与所有 `Camera`
  都释放后 SDK 反初始化，此后 `Sdk::new` 返回 `Error::Finalized`。
- **取流**：`Camera::start_grabbing` 与 `start_grabbing_with` 返回借用相机的守卫；需要把守卫存进结构体时，
  用 `Grabbing::start(camera)` 或 `CallbackGrabbing::start(camera, callback)` 按值持有相机，开始失败或 `stop` 时交还相机。
  守卫释放时停止取流，取流期间仍可读写节点。
- **Callback**：在 SDK 的线程中运行，其中的 panic 会终止进程。不要在 callback 里关闭相机或停止取流，
  应通过 channel 交给持有相机的线程。
- **清理**：`Drop` 忽略清理错误；需要检查时调用 `Camera::close`、`Grabbing::stop` 或 `CallbackGrabbing::stop`。
- **字符串**：节点名等参数使用 `&CStr`，例如 `c"ExposureTime"`。

细节见 [`docs::architecture`](https://docs.rs/mvs-sdk/latest/mvs_sdk/docs/architecture/index.html)。

## 已封装的接口

以 MVS 4.7.0 的 `MvCameraControl.h` 为准（不含 `MvObsoleteInterfaces.h` 中的废弃接口），144 个接口已封装
31 个。其余接口可以经 `mvs_sdk::sys` 调用，handle 与设备记录分别由 `Camera::as_raw_handle` 与
`DeviceInfo::as_raw` 提供。

| Part | 接口分组 | 接口数 | 已封装 |
| --- | --- | ---: | ---: |
| 1 | SDK 初始化与版本 | 3 | 3 |
| 2 | 相机控制与取流 | 34 | 12 |
| 3 | 采集卡配置 | 6 | 0 |
| 4 | 相机/采集卡通用属性 | 28 | 12 |
| 5 | 相机和采集卡升级 | 2 | 0 |
| 6 | 异常 callback 与事件 | 6 | 4 |
| 7 | GigE 专用接口 | 21 | 0 |
| 8 | CameraLink 专用接口 | 6 | 0 |
| 9 | U3V 专用接口 | 7 | 0 |
| 10 | GenTL 接口 | 4 | 0 |
| 11 | 图像保存、转换、处理与录像 | 22 | 0 |
| 12 | 串口通信 | 5 | 0 |

SDK 调用失败时返回 `Error::Sdk { function, code }`，`code` 的类型是 `ErrorCode`。

| Part | MVS SDK 接口 | Rust 接口 | 说明 |
| --- | --- | --- | --- |
| 1 | `MV_CC_Initialize` | `Sdk::new() -> Result<Sdk>` | 首次调用时初始化 |
| 1 | `MV_CC_Finalize` | `Sdk` 与所有 `Camera` 释放时自动调用 |  |
| 1 | `MV_CC_GetSDKVersion` | `Sdk::version() -> u32` | 无需初始化 |
| 2 | `MV_CC_EnumDevices` | `Sdk::devices(&self, TransportLayer) -> Result<Vec<DeviceInfo>>` |  |
| 2 | `MV_CC_IsDeviceAccessible` | `Sdk::is_accessible(&self, &DeviceInfo, AccessMode) -> bool` |  |
| 2 | `MV_CC_CreateHandle`、`MV_CC_OpenDevice` | `Sdk::open(&self, &DeviceInfo, AccessMode, u16) -> Result<Camera>` | `u16` 是切换 key，只对原生 GigE 设备有意义 |
| 2 | `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` |  |
| 2 | `MV_CC_CloseDevice`、`MV_CC_DestroyHandle` | `Camera::close(self) -> Result<()>`、`Drop` |  |
| 2 | `MV_CC_RegisterImageCallBackEx2` | `CallbackGrabbing::start(C, F) -> Result<CallbackGrabbing<C>, (C, Error)>`、`Camera::start_grabbing_with(&mut self, F) -> Result<CallbackGrabbing<&mut Camera>>` | `C: HoldsCamera`（`Camera` 或 `&mut Camera`）；`F: Fn(Frame<'_>) + Send + Sync + 'static` |
| 2 | `MV_CC_StartGrabbing` | `Grabbing::start(C) -> Result<Grabbing<C>, (C, Error)>`、`Camera::start_grabbing(&mut self) -> Result<Grabbing<&mut Camera>>`、`CallbackGrabbing::start` | 失败时交还相机 |
| 2 | `MV_CC_StopGrabbing` | `Grabbing::stop(self) -> (C, Result<()>)`、`CallbackGrabbing::stop(self) -> (C, Result<()>)`、守卫的 `Drop` | 交还相机 |
| 2 | `MV_CC_GetImageBuffer` | `Grabbing::get_image_buffer(&self, Option<Duration>) -> Result<FrameGuard<'_>>` | `None` 表示无限等待 |
| 2 | `MV_CC_FreeImageBuffer` | `FrameGuard` 的 `Drop` |  |
| 4 | `MV_CC_GetIntValueEx` | `Camera::get_int(&self, &CStr) -> Result<IntValue>` |  |
| 4 | `MV_CC_SetIntValueEx` | `Camera::set_int(&self, &CStr, i64) -> Result<()>` |  |
| 4 | `MV_CC_GetEnumValueEx` | `Camera::get_enum(&self, &CStr) -> Result<EnumValue>` |  |
| 4 | `MV_CC_SetEnumValue` | `Camera::set_enum(&self, &CStr, u32) -> Result<()>` |  |
| 4 | `MV_CC_SetEnumValueByString` | `Camera::set_enum_symbolic(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_GetFloatValue` | `Camera::get_float(&self, &CStr) -> Result<FloatValue>` |  |
| 4 | `MV_CC_SetFloatValue` | `Camera::set_float(&self, &CStr, f32) -> Result<()>` |  |
| 4 | `MV_CC_GetBoolValue` | `Camera::get_bool(&self, &CStr) -> Result<bool>` |  |
| 4 | `MV_CC_SetBoolValue` | `Camera::set_bool(&self, &CStr, bool) -> Result<()>` |  |
| 4 | `MV_CC_GetStringValue` | `Camera::get_string(&self, &CStr) -> Result<StringValue>` |  |
| 4 | `MV_CC_SetStringValue` | `Camera::set_string(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_SetCommandValue` | `Camera::execute_command(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_RegisterExceptionCallBack` | `Camera::register_exception_callback(&mut self, F)`、`unregister_exception_callback(&mut self)` | `F: Fn(ExceptionKind) + Send + Sync + 'static` |
| 6 | `MV_CC_RegisterEventCallBackEx` | `Camera::register_event_callback(&mut self, &CStr, F)`、`unregister_event_callback(&mut self, &CStr)` | `F: Fn(EventInfo<'_>) + Send + Sync + 'static` |
| 6 | `MV_CC_EventNotificationOn` | `Camera::event_notification_on(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_EventNotificationOff` | `Camera::event_notification_off(&self, &CStr) -> Result<()>` |  |

## 结构体

| MVS SDK 结构体 | Rust 类型 | 说明 |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO` | `DeviceInfo` | 字符串为 `&CStr`，GigE 地址为 `Ipv4Addr`，MAC 为 `[u8; 6]` |
| `MV_FRAME_OUT` | `FrameGuard<'_>`、`Frame<'_>` | `FrameGuard` 释放时归还 buffer；`Frame` 借用像素，需要保留时复制 `data` |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo` | 宽高优先取扩展字段，支持大于 65535 的尺寸 |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | 只在本次回调中有效 |
| `MVCC_INTVALUE_EX` | `IntValue` |  |
| `MVCC_FLOATVALUE` | `FloatValue` |  |
| `MVCC_ENUMVALUE_EX` | `EnumValue` |  |
| `MVCC_STRINGVALUE` | `StringValue` |  |

## 许可证

MIT。MVS SDK 的使用受海康机器人的许可条款约束，本项目不分发 SDK 文件。
