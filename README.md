# mvs-sdk-rs

海康威视机器人 MVS 工业相机 SDK 的安全 Rust 封装。workspace 包含：

- `mvs-sdk-rs`：面向应用的安全接口；
- `mvs-sdk-sys`：bindgen 生成的原始 FFI。

真实相机访问支持 Windows x86_64 MSVC。其它目标保留同形 API，`Sdk::initialize` 返回
`MvsError::UnsupportedPlatform`。项目暂不发布到 Crates.io，请通过仓库源码或本地
`path` 依赖使用。

## 环境

- 安装 MVS SDK；
- `MVCAM_COMMON_RUNENV` 指向 SDK 的 `Development` 目录；
- MVS DLL 目录加入运行时 `PATH`。

常用调用顺序：

```text
Sdk::version（可选） → Sdk::initialize → devices(layers) → Sdk::open(device, mode, key)
                  → 配置节点 → callback 或 polling → stop_grabbing
                  → Camera::close → Sdk::shutdown
```

`Sdk`、`Camera` 通过内部 `Arc<RuntimeCore>` 共享一次性 native session。`Camera` 不借用
公开 `Sdk`，可以直接放入业务结构或移动到 worker thread；每个 Camera 仍唯一拥有自己的
native handle。该骨架与 3dmvs wrapper 对称，MVS 独有的 `DestroyHandle` 和零拷贝 polling
buffer 继续按厂商接口表达。

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
| 1 | `MV_CC_Initialize` | `Sdk::initialize() -> MvsResult<Sdk>` | 每个进程最多尝试一次 Initialize。 |
| 1 | `MV_CC_Finalize` | `Sdk::shutdown(self) -> MvsResult<()>` | 仅在其它 session owner 已释放且无 orphan handle 时单次尝试 Finalize。 |
| 1 | `MV_CC_GetSDKVersion` | `Sdk::version() -> MvsResult<u32>` | 独立查询，可在 Initialize 前调用。 |
| 2 | `MV_CC_EnumDevices` | `Sdk::devices(&self, TransportLayer) -> MvsResult<Vec<DeviceInfo>>` | 串行枚举并深拷贝 owned snapshot；transport mask 直接转发。 |
| 2 | `MV_CC_IsDeviceAccessible` | `Sdk::is_accessible(&self, &DeviceInfo, AccessMode) -> bool` | 在活动 session 中使用 owned snapshot 查询。 |
| 2 | `MV_CC_CreateHandle` | `Sdk::open(&self, &DeviceInfo, AccessMode, u16) -> MvsResult<Camera>` | 与 `OpenDevice` 合并；失败时回滚非空 handle，成功但返回空 handle 时报告 `MvsError::NullHandleAfterCreate`。 |
| 2 | `MV_CC_OpenDevice` | `Sdk::open(&self, &DeviceInfo, AccessMode, u16) -> MvsResult<Camera>` | Camera 取得内部 session lease；回滚销毁也失败时 `OpenRollback` 保留两项错误。 |
| 2 | `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` | 返回调用时的连接状态快照。 |
| 2 | `MV_CC_CloseDevice` | `Camera::close(self) -> Result<(), CleanupError>` | 单次完整清理；保留首个失败操作、对应错误与独立的 Destroy 错误。 |
| 2 | `MV_CC_DestroyHandle` | `Camera::close(self) -> Result<(), CleanupError>`；`Drop` 兜底 | 销毁成功才确认 handle 与 callback backing 释放。 |
| 2 | `MV_CC_RegisterImageCallBackEx2` | `register_image_callback(F)` / `unregister_image_callback()` | `F: Fn(&Frame<'_>) + Send + Sync + 'static`；固定 `bAutoFree=true`。 |
| 2 | `MV_CC_StartGrabbing` | `Camera::start_grabbing(&mut self) -> MvsResult<()>` | image callback 与 polling 二选一。 |
| 2 | `MV_CC_StopGrabbing` | `Camera::stop_grabbing(&mut self) -> MvsResult<()>` | 变更 image callback 或切换取流方式前先停止。 |
| 2 | `MV_CC_GetImageBuffer` | `Camera::get_image_buffer(u32)`、`get_image_buffer_blocking()`、`get_owned_frame(u32)`、`get_owned_frame_blocking()` | 有限等待拒绝 `u32::MAX`；blocking 入口转发 SDK 无限等待哨兵。零拷贝 guard 借用相机；owned 入口复制后显式归还。 |
| 2 | `MV_CC_FreeImageBuffer` | `FrameGuard::release(self)`、`Camera::get_owned_frame`、`Drop` 兜底 | 显式路径传播归还错误；`Drop` 忽略错误。 |
| 4 | `MV_CC_GetIntValueEx` | `Camera::get_int(&self, &str) -> MvsResult<IntValue>` | 返回当前值、上下界和步长。 |
| 4 | `MV_CC_SetIntValueEx` | `Camera::set_int(&self, &str, i64) -> MvsResult<()>` | 直接转发 `i64`。 |
| 4 | `MV_CC_GetEnumValueEx` | `Camera::get_enum(&self, &str) -> MvsResult<EnumValue>` | 复制当前值和最多 256 个候选值。 |
| 4 | `MV_CC_SetEnumValue` | `Camera::set_enum_value(&self, &str, u32) -> MvsResult<()>` | 按 numeric value 设置。 |
| 4 | `MV_CC_SetEnumValueByString` | `Camera::set_enum_symbolic(&self, &str, &str) -> MvsResult<()>` | 按 symbolic value 设置。 |
| 4 | `MV_CC_GetFloatValue` | `Camera::get_float(&self, &str) -> MvsResult<FloatValue>` | 返回当前值和上下界。 |
| 4 | `MV_CC_SetFloatValue` | `Camera::set_float(&self, &str, f32) -> MvsResult<()>` | 直接转发 `f32`。 |
| 4 | `MV_CC_GetBoolValue` | `Camera::get_bool(&self, &str) -> MvsResult<bool>` | 转换 SDK `bool_`。 |
| 4 | `MV_CC_SetBoolValue` | `Camera::set_bool(&self, &str, bool) -> MvsResult<()>` | 转换 Rust `bool`。 |
| 4 | `MV_CC_GetStringValue` | `Camera::get_string(&self, &str) -> MvsResult<SdkText>` | 保留 SDK 原始字节。 |
| 4 | `MV_CC_SetStringValue` | `Camera::set_string(&self, &str, &[u8]) -> MvsResult<()>` | 检查内部 NUL。 |
| 4 | `MV_CC_SetCommandValue` | `Camera::exec_command(&self, &str) -> MvsResult<()>` | 执行 GenICam command 节点。 |
| 6 | `MV_CC_RegisterExceptionCallBack` | `register_exception_callback(F)` / `unregister_exception_callback()` | `F: Fn(u32) + Send + Sync + 'static`。 |
| 6 | `MV_CC_RegisterEventCallBackEx` | `register_event_callback(&str, F)` / `unregister_event_callback(&str)` | `F: Fn(&EventInfo<'_>) + Send + Sync + 'static`。 |
| 6 | `MV_CC_EventNotificationOn` | `Camera::event_notification_on(&self, &str) -> MvsResult<()>` | 启用设备端指定事件。 |
| 6 | `MV_CC_EventNotificationOff` | `Camera::event_notification_off(&self, &str) -> MvsResult<()>` | callback 注册与设备端开关独立。 |

## SDK 结构体与 Rust 结构体

| MVS SDK 结构体 | Rust 定义 | 转换与生命周期 |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO_LIST` | `Vec<DeviceInfo>` | 枚举锁内复制全部有效设备项，返回后独立于 SDK 临时列表。 |
| `MV_CC_DEVICE_INFO` 及 `SpecialInfo` | `DeviceInfo`、`TransportLayer`、`SdkText` | 公开常用字段；字符串保留原始字节。CreateHandle 仍使用内部 C 快照。 |
| `MV_FRAME_OUT` | `FrameGuard<'cam>`、`Frame<'_>` | guard 保存释放凭据并借用相机；`Frame` 借用像素区。 |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo`、`PixelType` | 复制常用字段子集：尺寸、长度、编号、像素格式、增益、曝光和时间戳等。 |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | callback 期间借用事件名原始字节并复制数值字段。 |
| `MVCC_INTVALUE_EX` | `IntValue` | 复制当前值、上下界和步长。 |
| `MVCC_FLOATVALUE` | `FloatValue` | 复制当前值和上下界。 |
| `MVCC_ENUMVALUE_EX` | `EnumValue` | 复制当前值和最多 256 个候选值。 |
| `MVCC_STRINGVALUE` | `SdkText` | 按字段容量读取原始字节。 |

`OwnedFrame` 是 `Frame` 的 Rust-owned 像素副本，生命周期独立于 SDK buffer；
`Camera::get_owned_frame` 提供 polling 获取、复制和显式归还的一步入口。

## 生命周期约束

完整数据流见
[`Callback 取流`](时序图/Callback取流.md)、
[`轮询取图与 buffer 归还`](时序图/轮询取图与buffer归还.md)、
[`Camera 显式关闭与 Drop 兜底`](时序图/Camera显式关闭与Drop兜底.md) 和
[`Sdk shutdown 的终态约束`](时序图/Sdk-shutdown的终态约束.md)。

- `Sdk` 是唯一显式 Finalize 入口；`Sdk` 与每个 `Camera` 通过 `Arc<RuntimeCore>` 持有 session lease，`shutdown(self)` 先以 `Arc::try_unwrap` 检查其它 owner。`DeviceInfo` 是纯 owned snapshot，不参与 session 生命周期。
- 官方 CHM 限定单进程只执行一次 Initialize 与 Finalize；Windows x86_64 MSVC 的 Initialize 机会一经声明即不复位，Initialize 失败、`Sdk` 普通 Drop、shutdown 成功或失败后均不支持同进程重启。unsupported 目标不调用 native 接口，每次均返回 `UnsupportedPlatform`。
- `Sdk` 普通 Drop 跳过 Finalize；显式 `shutdown(self)` 在其它 session owner 存活时返回 `MvsError::InvalidState`，调用方按终止进程处理。
- `CreateHandle` 写出非空 handle 后即计为 live，计数记在 `RuntimeCore` 上，只有 `DestroyHandle` 成功才解除。普通 Camera owner 由 Arc 门禁；owner 已消费而计数仍为 live 时，`Sdk::shutdown` 返回 `MvsError::NativeHandlesLive`。
- Stop、callback 注销或 `CloseDevice` 失败不阻断后续 `DestroyHandle`；Destroy 成功后允许 Finalize，Destroy 失败才进入进程终止分支。这与只有 Close 终点的 3dmvs wrapper 是厂商契约导致的合理差异。
- image callback 使用 `RegisterImageCallBackEx2` 且 `bAutoFree=true`，`Frame` 只在 callback 调用期间有效。
- image callback 与 polling 互斥；注册、注销或切换方式前停止采集。
- callback 使用 `Fn + Send + Sync`。Camera-owned `Box` 固定 `pUser` 地址；注销返回时，已进入的 callback 可能仍在执行，closure `Arc` 保活到该次调用结束。当前线程位于任一 MVS callback 时，start/stop/register 返回 `InvalidState`；`close` / `Drop` 终止进程。生命周期变更通过 channel 通知 owner 线程处理。
- wrapper 不增加 callback drain；普通 owner teardown 依赖 SDK 的 Stop、Close、Destroy 同步约定，`Arc` 只保活已经进入 Rust 的 closure。
- 取流与 callback 注册状态只在 native 返回 `MV_OK` 后更新；失败保留调用前的本地状态并返回原错误。仍持有 owner 的普通操作由调用方决定重试；首次注册失败回收新建 slot，已有 record 注册失败时清空 closure，注销后可重新注册。
- callback 的业务错误通过 channel 交给 owner；panic 在 FFI 边界终止进程。
- polling 有限等待使用 `u32` 毫秒并拒绝 `u32::MAX`；无限等待使用 `get_image_buffer_blocking` / `get_owned_frame_blocking`。
- polling buffer 由 `FrameGuard` 唯一归还；`release(self)` 单次尝试并返回错误，`Drop` 兜底时忽略错误。`get_owned_frame` 复用该流程并传播 release 错误。
- `Camera::close(self)` 与 `Sdk::shutdown(self)` 都会消费 owner 并只尝试一次，错误用于诊断和宿主退出策略，不能用同一 owner 重试。Camera 的 Drop 执行局部清理兜底；Sdk 的 Drop 跳过 Finalize。`OpenRollback` 保留 open/create 与回滚销毁错误；`CleanupError` 保留首个失败操作、对应错误与独立的 DestroyHandle 错误。
- `unsafe Camera::as_raw_handle` 与 `unsafe DeviceInfo::as_raw` 只借出指针；raw 调用不得改变 safe 层维护的取流、callback 或 handle 生命周期。

## 文档与验证

```console
cargo doc --workspace --no-deps --open
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

真机测试需要 Windows x64 MSVC、MVS SDK、专用相机和
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
