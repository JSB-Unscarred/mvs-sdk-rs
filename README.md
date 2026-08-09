# mvs-sdk-rs

海康威视机器人 MVS 工业相机 SDK 的安全 Rust 封装。workspace 包含：

- `mvs-sdk-rs`：面向应用的安全接口；
- `mvs-sdk-sys`：bindgen 生成的原始 FFI。

真实相机访问支持 Windows x86_64。其它目标保留同形 API，`Sdk::init` 返回
`MvsError::UnsupportedPlatform`。项目暂不发布到 Crates.io，请通过仓库源码或本地
`path` 依赖使用。

## 环境

- 安装 MVS SDK；
- `MVCAM_COMMON_RUNENV` 指向 SDK 的 `Development` 目录；
- MVS DLL 目录加入运行时 `PATH`。

常用调用顺序：

```text
Sdk::sdk_version（可选） → Sdk::init → enumerate_devices → open(mode, switchover_key)
                       → 配置节点 → callback 或 polling → stop_grabbing
                       → Camera::close → 释放 DeviceList → Sdk::shutdown
```

真实设备测试沿完整数据流集中在
[`tests/hardware_smoke.rs`](tests/hardware_smoke.rs)。运行时需启用 `hardware-tests`
feature，并指定专用测试相机。

## SDK 接口与安全 Rust 接口

审阅基准为 MVS V4.7.0 的 `MvCameraControl.h`。其中 Part 1-12 共定义 144 个
current API，`mvs-sdk-sys` 已生成全部 144 个 raw binding，safe crate 直接覆盖
31 个。其余 113 个按官方 Part 顺序待实现。

`MvCameraControl.h` 还包含 `MvObsoleteInterfaces.h`。其中 114 个 deprecated API
不计入 current API；bindings 生成脚本通过 blocklist 排除
`MvObsoleteInterfaces.h` 和 `ObsoleteCamParams.h`。deprecated 功能优先映射到 current
API，不扩展 safe 层。

### Part 覆盖统计

| 官方顺序 | 接口分组 | current API | safe 直接覆盖 | 待实现 |
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

### 已覆盖接口映射

下表只列 safe crate 已覆盖的接口，并严格沿用 `MvCameraControl.h` 的 Part 和声明顺序。

| Part | MVS SDK 接口 | 安全 Rust 接口定义 | 关键约束 |
| --- | --- | --- | --- |
| 1 | `MV_CC_Initialize` | `Sdk::init() -> MvsResult<Sdk>` | 进程内只创建一个 SDK owner。 |
| 1 | `MV_CC_Finalize` | `Sdk::shutdown(self) -> MvsResult<()>` | 消费 SDK owner；其借用资源需先结束。 |
| 1 | `MV_CC_GetSDKVersion` | `Sdk::sdk_version() -> MvsResult<u32>` | 独立查询，可在 `Sdk::init` 前调用。 |
| 2 | `MV_CC_EnumDevices` | `Sdk::enumerate_devices(&self, TransportLayer) -> MvsResult<DeviceList<'_>>` | 串行枚举、复制设备记录，返回值借用 `Sdk`。 |
| 2 | `MV_CC_IsDeviceAccessible` | `DeviceInfo::is_accessible(&self, AccessMode) -> bool` | 使用枚举记录的私有副本查询。 |
| 2 | `MV_CC_CreateHandle` | `DeviceInfo<'sdk>::open(&self, AccessMode, u16) -> MvsResult<Camera<'sdk>>` | 与 `OpenDevice` 合并，创建失败时回收 handle。 |
| 2 | `MV_CC_OpenDevice` | `DeviceInfo<'sdk>::open(&self, AccessMode, u16) -> MvsResult<Camera<'sdk>>` | `u16` 直接对应 `nSwitchoverKey`。 |
| 2 | `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` | 返回调用时的连接状态快照。 |
| 2 | `MV_CC_CloseDevice` | `Camera::close(self) -> MvsResult<()>` | 与 `DestroyHandle` 组成完整清理。 |
| 2 | `MV_CC_DestroyHandle` | `Camera::close(self) -> MvsResult<()>`；`Drop` 兜底 | native handle 只由 `Camera` owner 清理。 |
| 2 | `MV_CC_RegisterImageCallBackEx2` | `register_image_callback(F)` / `unregister_image_callback()` | `F: Fn(&Frame<'_>) + Send + Sync + 'static`；固定 `bAutoFree=true`。 |
| 2 | `MV_CC_StartGrabbing` | `Camera::start_grabbing(&mut self) -> MvsResult<()>` | image callback 与 polling 二选一。 |
| 2 | `MV_CC_StopGrabbing` | `Camera::stop_grabbing(&mut self) -> MvsResult<()>` | 变更 image callback 或切换取流方式前先停止。 |
| 2 | `MV_CC_GetImageBuffer` | `Camera::get_image_buffer(&self, u32) -> MvsResult<FrameGuard<'_>>` | guard 借用相机和 SDK buffer。 |
| 2 | `MV_CC_FreeImageBuffer` | `FrameGuard::release(self) -> MvsResult<()>`；`Drop` 兜底 | 每个成功取得的 buffer 只归还一次。 |
| 4 | `MV_CC_GetIntValueEx` | `Camera::get_int(&self, &str) -> MvsResult<IntValue>` | 返回当前值、上下界和步长。 |
| 4 | `MV_CC_SetIntValueEx` | `Camera::set_int(&self, &str, i64) -> MvsResult<()>` | 直接转发 `i64`。 |
| 4 | `MV_CC_GetEnumValueEx` | `Camera::get_enum(&self, &str) -> MvsResult<EnumValue>` | 复制当前值和最多 256 个候选值。 |
| 4 | `MV_CC_SetEnumValue` | `Camera::set_enum_value(&self, &str, u32) -> MvsResult<()>` | 按 numeric value 设置。 |
| 4 | `MV_CC_SetEnumValueByString` | `Camera::set_enum_symbolic(&self, &str, &str) -> MvsResult<()>` | 按 symbolic value 设置。 |
| 4 | `MV_CC_GetFloatValue` | `Camera::get_float(&self, &str) -> MvsResult<FloatValue>` | 返回当前值和上下界。 |
| 4 | `MV_CC_SetFloatValue` | `Camera::set_float(&self, &str, f32) -> MvsResult<()>` | 直接转发 `f32`。 |
| 4 | `MV_CC_GetBoolValue` | `Camera::get_bool(&self, &str) -> MvsResult<bool>` | 转换 SDK `bool_`。 |
| 4 | `MV_CC_SetBoolValue` | `Camera::set_bool(&self, &str, bool) -> MvsResult<()>` | 转换 Rust `bool`。 |
| 4 | `MV_CC_GetStringValue` | `Camera::get_string(&self, &str) -> MvsResult<String>` | 返回 owned `String`。 |
| 4 | `MV_CC_SetStringValue` | `Camera::set_string(&self, &str, &str) -> MvsResult<()>` | 检查内部 NUL。 |
| 4 | `MV_CC_SetCommandValue` | `Camera::exec_command(&self, &str) -> MvsResult<()>` | 执行 GenICam command 节点。 |
| 6 | `MV_CC_RegisterExceptionCallBack` | `register_exception_callback(F)` / `unregister_exception_callback()` | `F: Fn(u32) + Send + Sync + 'static`。 |
| 6 | `MV_CC_RegisterEventCallBackEx` | `register_event_callback(&str, F)` / `unregister_event_callback(&str)` | `F: Fn(&EventInfo<'_>) + Send + Sync + 'static`。 |
| 6 | `MV_CC_EventNotificationOn` | `Camera::event_notification_on(&self, &str) -> MvsResult<()>` | 启用设备端指定事件。 |
| 6 | `MV_CC_EventNotificationOff` | `Camera::event_notification_off(&self, &str) -> MvsResult<()>` | callback 注册与设备端开关独立。 |

## SDK 结构体与 Rust 结构体

| MVS SDK 结构体 | Rust 定义 | 转换与生命周期 |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO_LIST` | `DeviceList<'sdk>` | 复制有效设备项并借用 `Sdk`；`iter()` 返回 `&DeviceInfo<'sdk>`。 |
| `MV_CC_DEVICE_INFO` 及 `SpecialInfo` | `DeviceInfo<'sdk>`、`TransportLayer` | 保存 Rust-owned snapshot，通过访问器读取 metadata。 |
| `MV_FRAME_OUT` | `FrameGuard<'cam>`、`Frame<'_>` | guard 保存释放凭据并借用相机；`Frame` 借用像素区。 |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo`、`PixelType` | 复制常用字段子集：尺寸、长度、编号、像素格式、增益、曝光和时间戳等。 |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | callback 期间借用事件名并复制数值字段。 |
| `MVCC_INTVALUE_EX` | `IntValue` | 复制当前值、上下界和步长。 |
| `MVCC_FLOATVALUE` | `FloatValue` | 复制当前值和上下界。 |
| `MVCC_ENUMVALUE_EX` | `EnumValue` | 复制当前值和最多 256 个候选值。 |
| `MVCC_STRINGVALUE` | `String` | 按字段容量读取并生成 owned 字符串。 |

`OwnedFrame` 是 `Frame` 的 Rust-owned 像素副本，生命周期独立于 SDK buffer。

## 生命周期约束

- `Sdk` 是进程级唯一 owner；`DeviceList<'sdk>`、`DeviceInfo<'sdk>` 和 `Camera<'sdk>` 均借用它，借用结束后才能消费 `Sdk` 调用 `shutdown`。
- 官方 CHM 限定单进程只执行一次 Initialize 与 Finalize；Initialize 失败或 Finalize 尝试后均进入终态，后续 `Sdk::init` 返回 `SdkTerminated`。
- `DeviceList::iter()` 借出列表内的 `DeviceInfo`，枚举记录由列表统一持有。
- image callback 使用 `RegisterImageCallBackEx2` 且 `bAutoFree=true`，`Frame` 只在 callback 调用期间有效。
- image callback 与 polling 互斥；注册、注销或切换方式前停止采集。
- callback 使用 `Fn + Send + Sync`。注销返回时，已进入的 callback 可能仍在执行；`Arc` backing 持有 closure 到该次调用结束。
- polling buffer 由 `FrameGuard` 唯一归还；显式 `release` 可取得错误，`Drop` 执行兜底。
- 正常路径显式调用 `Camera::close` 和 `Sdk::shutdown`，各 owner 的 `Drop` 只清理自己的局部资源。

## 文档与验证

```console
cargo doc --workspace --no-deps --open
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
```

真机测试需要 Windows x64、MVS SDK、专用相机和
`MVS_TEST_CAMERA_SERIAL`：

```console
cargo test --features hardware-tests --tests -- --ignored
```

## 更新 bindings

升级 MVS SDK 后运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

脚本读取 `MVCAM_COMMON_RUNENV`，并要求 LLVM/libclang 与 `bindgen-cli 0.72.1`。

## License

MIT
