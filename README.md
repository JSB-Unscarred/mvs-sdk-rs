# mvs-sdk-rs

海康威视机器人 MVS 工业相机 SDK 的安全 Rust 封装。workspace 包含：

- `mvs-sdk-rs`：安全接口；
- `mvs-sdk-sys`：bindgen 生成的原始 FFI。

支持 Windows x86_64 MSVC。项目暂不发布到 Crates.io，请通过仓库源码或本地 `path` 依赖使用。

## 环境

- 安装 MVS SDK；
- `MVCAM_COMMON_RUNENV` 指向 SDK 的 `Development` 目录；
- MVS DLL 目录加入运行时 `PATH`。

## 快速开始

```rust,no_run
use std::time::Duration;

use mvs_sdk_rs::{AccessMode, Sdk, TransportLayer};

fn main() -> mvs_sdk_rs::Result<()> {
    let sdk = Sdk::initialize()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let mut camera = sdk.open(&devices[0], AccessMode::Exclusive, 0)?;
    camera.set_float(c"ExposureTime", 10_000.0)?;

    let grabbing = camera.start_grabbing()?;
    let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
    println!("{:?}", buffer.frame());
    Ok(())
}
```

更多用法见 [`examples/`](examples)：`enumerate_devices`、`grab_polling`、`grab_callback`。

## 生命周期

所有权、类型状态、清理失败策略和 callback 约定集中在 rustdoc 的
[`docs::architecture`](src/docs/architecture.rs)。要点：

- `Sdk` 与每个 `Camera` 共享 SDK 会话，全部释放后自动 `MV_CC_Finalize`；每个进程只能初始化一次。
- `Camera::start_grabbing` / `start_grabbing_with` 返回可变借用相机的守卫，polling 取图只存在于
  `Grabbing` 上；守卫释放时停止取流。
- `Drop` 忽略清理错误；需要观察时调用 `Camera::close`、`Grabbing::stop` 或 `CallbackGrabbing::stop`。
- 字符串参数使用 `&CStr`，例如 `c"ExposureTime"`。

## SDK 接口与安全 Rust 接口

审阅基准为 MVS V4.7.0 的 `MvCameraControl.h`。其中 Part 1-12 共定义 144 个 current API，
`mvs-sdk-sys` 已生成全部 raw binding，安全接口覆盖 31 个。`MvObsoleteInterfaces.h` 中的
deprecated API 不计入，bindings 生成脚本通过 blocklist 排除。未封装的接口可通过
`Camera::as_raw_handle` 与 `DeviceInfo::as_raw` 调用 `mvs-sdk-sys`。

### Part 覆盖统计

| 官方顺序 | 接口分组 | current API | 已封装 | 待实现 |
| --- | --- | ---: | ---: | ---: |
| Part 1 | SDK 初始化与版本 | 3 | 3 | 0 |
| Part 2 | 相机控制与取流 | 34 | 12 | 22 |
| Part 3 | 采集卡配置 | 6 | 0 | 6 |
| Part 4 | 相机/采集卡通用属性 | 28 | 12 | 16 |
| Part 5 | 相机和采集卡升级 | 2 | 0 | 2 |
| Part 6 | 异常 callback 与事件 | 6 | 4 | 2 |
| Part 7 | GigE 专用接口 | 21 | 0 | 21 |
| Part 8 | CameraLink 专用接口 | 6 | 0 | 6 |
| Part 9 | U3V 专用接口 | 7 | 0 | 7 |
| Part 10 | GenTL 接口 | 4 | 0 | 4 |
| Part 11 | 图像保存、转换、处理与录像 | 22 | 0 | 22 |
| Part 12 | 串口通信 | 5 | 0 | 5 |
| **合计** |  | **144** | **31** | **113** |

### 已封装接口

按 `MvCameraControl.h` 的 Part 与声明顺序排列。所有 `Result` 的错误均为 `Error::Sdk { function, code }`，
另有 `Error::AlreadyInitialized`。

| Part | MVS SDK 接口 | 安全 Rust 接口 | 说明 |
| --- | --- | --- | --- |
| 1 | `MV_CC_Initialize` | `Sdk::initialize() -> Result<Sdk>` | 每个进程一次 |
| 1 | `MV_CC_Finalize` | `Sdk` 与全部 `Camera` 释放时自动调用 | 某个 handle 销毁失败后不再调用 |
| 1 | `MV_CC_GetSDKVersion` | `Sdk::version() -> u32` | 无需初始化 |
| 2 | `MV_CC_EnumDevices` | `Sdk::devices(&self, TransportLayer) -> Result<Vec<DeviceInfo>>` | 串行枚举并复制记录 |
| 2 | `MV_CC_IsDeviceAccessible` | `Sdk::is_accessible(&self, &DeviceInfo, AccessMode) -> bool` |  |
| 2 | `MV_CC_CreateHandle` | `Sdk::open(&self, &DeviceInfo, AccessMode, u16) -> Result<Camera>` | 与 `OpenDevice` 合并 |
| 2 | `MV_CC_OpenDevice` | 同上 | 失败时销毁 handle |
| 2 | `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` |  |
| 2 | `MV_CC_CloseDevice` | `Camera::close(self) -> Result<()>`、`Drop` | 返回首个错误 |
| 2 | `MV_CC_DestroyHandle` | 同上 | 失败时保留闭包与会话引用 |
| 2 | `MV_CC_RegisterImageCallBackEx2` | `Camera::start_grabbing_with(&mut self, F) -> Result<CallbackGrabbing<'_>>` | `F: Fn(&Frame<'_>) + Send + Sync + 'static`；`bAutoFree=true`；守卫结束时注销 |
| 2 | `MV_CC_StartGrabbing` | `Camera::start_grabbing(&mut self) -> Result<Grabbing<'_>>`、`start_grabbing_with` |  |
| 2 | `MV_CC_StopGrabbing` | `Grabbing::stop(self)`、`CallbackGrabbing::stop(self)`、守卫的 `Drop` |  |
| 2 | `MV_CC_GetImageBuffer` | `Grabbing::get_image_buffer(&self, Option<Duration>) -> Result<FrameGuard<'_>>` | `None` 为无限等待 |
| 2 | `MV_CC_FreeImageBuffer` | `FrameGuard` 的 `Drop` |  |
| 4 | `MV_CC_GetIntValueEx` | `Camera::get_int(&self, &CStr) -> Result<IntValue>` |  |
| 4 | `MV_CC_SetIntValueEx` | `Camera::set_int(&self, &CStr, i64) -> Result<()>` |  |
| 4 | `MV_CC_GetEnumValueEx` | `Camera::get_enum(&self, &CStr) -> Result<EnumValue>` |  |
| 4 | `MV_CC_SetEnumValue` | `Camera::set_enum_value(&self, &CStr, u32) -> Result<()>` |  |
| 4 | `MV_CC_SetEnumValueByString` | `Camera::set_enum_symbolic(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_GetFloatValue` | `Camera::get_float(&self, &CStr) -> Result<FloatValue>` |  |
| 4 | `MV_CC_SetFloatValue` | `Camera::set_float(&self, &CStr, f32) -> Result<()>` |  |
| 4 | `MV_CC_GetBoolValue` | `Camera::get_bool(&self, &CStr) -> Result<bool>` |  |
| 4 | `MV_CC_SetBoolValue` | `Camera::set_bool(&self, &CStr, bool) -> Result<()>` |  |
| 4 | `MV_CC_GetStringValue` | `Camera::get_string(&self, &CStr) -> Result<CString>` |  |
| 4 | `MV_CC_SetStringValue` | `Camera::set_string(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_SetCommandValue` | `Camera::exec_command(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_RegisterExceptionCallBack` | `Camera::register_exception_callback(&mut self, F)`、`unregister_exception_callback(&mut self)` | `F: Fn(ExceptionKind) + Send + Sync + 'static`；闭包保留到 `DestroyHandle` |
| 6 | `MV_CC_RegisterEventCallBackEx` | `Camera::register_event_callback(&mut self, &CStr, F)`、`unregister_event_callback(&mut self, &CStr)` | `F: Fn(&EventInfo<'_>) + Send + Sync + 'static`；闭包保留到 `DestroyHandle` |
| 6 | `MV_CC_EventNotificationOn` | `Camera::event_notification_on(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_EventNotificationOff` | `Camera::event_notification_off(&self, &CStr) -> Result<()>` |  |

## SDK 结构体与 Rust 类型

| MVS SDK 结构体 | Rust 类型 | 说明 |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO_LIST` | `Vec<DeviceInfo>` | 枚举锁内复制有效项 |
| `MV_CC_DEVICE_INFO` 及 `SpecialInfo` | `DeviceInfo` | 保存原始记录；字符串按 transport 读取为 `&CStr`，`GigE` 地址为 `Ipv4Addr` |
| `MV_FRAME_OUT` | `FrameGuard<'_>`、`Frame<'_>` | 守卫负责归还 buffer；`Frame` 借用像素 |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo` | 复制常用字段，尺寸优先取扩展字段 |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | 回调期间借用事件名 |
| `MVCC_INTVALUE_EX` | `IntValue` |  |
| `MVCC_FLOATVALUE` | `FloatValue` |  |
| `MVCC_ENUMVALUE_EX` | `EnumValue` | 复制有效的候选值 |
| `MVCC_STRINGVALUE` | `CString` |  |

## 验证

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
cargo test --workspace
```

`clippy` 与 `doc` 不需要 SDK；`cargo test` 需要链接并运行 MVS DLL。真机测试需要专用相机与
`MVS_TEST_CAMERA_SERIAL`：

```powershell
cargo test --features hardware-tests --test hardware_smoke -- --ignored
```

## 更新 bindings

升级 MVS SDK 后运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

脚本读取 `MVCAM_COMMON_RUNENV`，需要 LLVM/libclang 与 `bindgen-cli 0.72.1`。

## License

MIT
