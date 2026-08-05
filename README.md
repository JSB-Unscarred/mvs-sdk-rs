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
Sdk::init → enumerate_devices → open → 配置节点 → callback 或 polling
          → stop_grabbing → Camera::close → Sdk::shutdown
```

完整示例见 [`examples/callback.rs`](examples/callback.rs) 和
[`examples/polling.rs`](examples/polling.rs)。运行示例需启用 `hardware-tests` feature。

## SDK 接口与安全 Rust 接口

本表只维护 safe crate 已覆盖的 MVS SDK 接口。

| MVS SDK 接口 | 安全 Rust 接口定义 | 关键约束 |
| --- | --- | --- |
| `MV_CC_Initialize` | `Sdk::init() -> MvsResult<Arc<Sdk>>` | 串行化进程级初始化并复用同一 `Sdk`。 |
| `MV_CC_Finalize` | `Sdk::shutdown(&self) -> Result<(), ShutdownError>` | 相机和 callback 退出后终止；成功后进入进程终态。 |
| `MV_CC_GetSDKVersion` | `Sdk::sdk_version(&self) -> u32` | 返回初始化时保存的版本。 |
| `MV_CC_EnumDevices` | `Sdk::enumerate_devices(&self, TransportLayer) -> MvsResult<DeviceList>` | 串行枚举并复制设备记录。 |
| `MV_CC_IsDeviceAccessible` | `DeviceInfo::is_accessible(&self, AccessMode) -> MvsResult<bool>` | 使用私有副本执行 C 查询。 |
| `MV_CC_CreateHandle` + `MV_CC_OpenDevice` | `DeviceInfo::open(&self, AccessMode) -> MvsResult<Camera>` | 创建并打开 handle；失败时回滚销毁。另提供 `open_exclusive`、`open_control`。 |
| `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` | 返回调用时的连接状态快照。 |
| `MV_CC_StartGrabbing` / `MV_CC_StopGrabbing` | `Camera::start_grabbing(&mut self)` / `stop_grabbing(&mut self) -> MvsResult<()>` | 检查采集状态并固定 callback 或 polling 模式。 |
| `MV_CC_GetImageBuffer` + `MV_CC_FreeImageBuffer` | `Camera::get_image_buffer(&self, u32) -> MvsResult<FrameGuard<'_>>`；`FrameGuard::release(self) -> MvsResult<()>` | guard 借用相机并成对释放 buffer，`Drop` 负责兜底。 |
| `MV_CC_RegisterImageCallBackEx` | `register_image_callback(FnMut(&Frame<'_>))` / `unregister_image_callback()` | `Frame` 仅在调用期间有效；注销等待在途 callback。 |
| `MV_CC_RegisterExceptionCallBack` | `register_exception_callback(FnMut(u32))` / `unregister_exception_callback()` | closure panic 在 FFI 边界截获。 |
| `MV_CC_RegisterEventCallBackEx` | `register_event_callback(&str, FnMut(&EventInfo<'_>))` / `unregister_event_callback(&str)` | 事件数据仅在调用期间借用；按名称管理注册。 |
| `MV_CC_EventNotificationOn` / `MV_CC_EventNotificationOff` | `event_notification_on(&self, &str)` / `event_notification_off(&self, &str)` | 设备端通知与 Rust callback 注册独立。 |
| `MV_CC_SetIntValueEx` / `MV_CC_GetIntValueEx` | `set_int`、`get_int`、`get_int_range` | 使用 `i64` 和 `IntNode`。 |
| `MV_CC_SetFloatValue` / `MV_CC_GetFloatValue` | `set_float`、`get_float`、`get_float_range` | 使用 `f32` 和 `FloatNode`。 |
| `MV_CC_SetBoolValue` / `MV_CC_GetBoolValue` | `set_bool`、`get_bool` | 转换 Rust `bool` 与 SDK `bool_`。 |
| `MV_CC_SetEnumValueByString` / `MV_CC_SetEnumValue` / `MV_CC_GetEnumValue` | `set_enum`、`set_enum_value`、`get_enum`、`get_enum_info` | 符号名优先；`EnumNode` 最多保存 64 个候选值。 |
| `MV_CC_SetStringValue` / `MV_CC_GetStringValue` | `set_string`、`get_string` | 检查内部 NUL，返回 owned `String`。 |
| `MV_CC_SetCommandValue` | `Camera::exec_command(&self, &str) -> MvsResult<()>` | 执行 GenICam command 节点。 |
| `MV_CC_CloseDevice` + `MV_CC_DestroyHandle` | `Camera::close(self)`；`close_detailed(self)` | 消费 `Camera` 并继续完整清理；分别返回首个或全部错误。 |

## SDK 结构体与 Rust 结构体

| MVS SDK 结构体 | Rust 定义 | 转换与生命周期 |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO_LIST` | `DeviceList`、`DeviceIter<'_>` | 复制有效设备项；迭代器借用 Rust-owned 列表。 |
| `MV_CC_DEVICE_INFO` 及 `SpecialInfo` | `DeviceInfo`、`TransportLayer` | 持有地址稳定的 owned snapshot，通过访问器读取 metadata。 |
| `MV_FRAME_OUT` | `FrameGuard<'cam>`、`Frame<'_>` | guard 保存释放凭据并借用相机；`Frame` 借用像素区。 |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo`、`PixelType` | 复制尺寸、编号、像素格式和时间戳等 metadata。 |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | callback 期间借用事件名并复制数值字段。 |
| `MVCC_INTVALUE_EX` | `IntNode` | 复制当前值、上下界和步长。 |
| `MVCC_FLOATVALUE` | `FloatNode` | 复制当前值和上下界。 |
| `MVCC_ENUMVALUE` | `EnumNode` | 复制当前值和候选值。 |
| `MVCC_STRINGVALUE` | `String` | 按字段容量读取并生成 owned 字符串。 |

`OwnedFrame` 是 `Frame` 的 Rust-owned 像素副本，生命周期独立于 SDK buffer。

## 生命周期约束

详细时序见
[`Callback取流`](时序图/Callback取流.md)、
[`轮询取图与buffer归还`](时序图/轮询取图与buffer归还.md)、
[`Camera显式关闭与Drop兜底`](时序图/Camera显式关闭与Drop兜底.md) 和
[`Sdk shutdown的终态约束`](时序图/Sdk-shutdown的终态约束.md)。

- image callback 与 polling 互斥；首次注册、注销或切换模式前停止采集。
- callback 应尽快返回；跨调用保存数据时先复制，image/event callback 内不关闭相机。
- `Camera` 是 `Send + !Sync`；`FrameGuard` 是 `!Send + !Sync`。
- 正常路径显式调用 `Camera::close`；`Drop` 只执行忽略错误的兜底清理。
- `as_raw` 和 `as_raw_handle` 只借出指针，不得绕过封装改变生命周期状态。
- 相机关闭且 callback 退出后调用 `Sdk::shutdown`；成功 shutdown 后进程内不再初始化。

## 文档与验证

```console
cargo doc --workspace --no-deps --open
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
```

真机 smoke test 需要 Windows x64、MVS SDK、专用相机和
`MVS_TEST_CAMERA_SERIAL`：

```console
cargo test --features hardware-tests --test hardware_smoke -- --ignored --test-threads=1
```

## 更新 bindings

升级 MVS SDK 后运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

脚本读取 `MVCAM_COMMON_RUNENV`，并要求 LLVM/libclang 与 `bindgen-cli 0.72.1`。

## License

MIT
