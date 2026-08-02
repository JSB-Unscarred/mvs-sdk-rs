# mvs-sdk-rs 项目协作指南

## 项目概述

本项目是海康威视机器人 MVS 工业相机 SDK 的 Rust 2024 安全封装，workspace 使用 Cargo resolver 3。

- 应用代码使用安全 crate `mvs-sdk-rs`；原始 FFI 由 `mvs-sdk-sys` 承载。
- Windows x86_64 使用真实 MVS SDK；其它目标保留同形 API，`Sdk::init()` 返回 `MvsError::UnsupportedPlatform`。
- 核心目标是用所有权、借用、状态机和显式关闭保证 SDK、相机、callback 与图像 buffer 的生命周期安全。
- 原生状态不确定时保守泄漏并阻止 SDK shutdown，不能释放厂商代码仍可能访问的内存。

## 项目架构

```text
.
├── Cargo.toml                 # workspace、安全 crate、feature 与测试配置
├── src/
│   ├── lib.rs                 # 公开 API 与重导出
│   ├── library.rs             # 进程级 Sdk 状态、资源账本、设备枚举锁
│   ├── device.rs              # DeviceList、DeviceInfo owned snapshot
│   ├── camera.rs              # Camera 公开 API 与清理入口
│   ├── frame.rs               # Frame、OwnedFrame、FrameGuard
│   ├── callback.rs            # EventInfo
│   ├── types.rs               # 访问模式、节点值、传输层、像素格式
│   ├── error.rs               # MvsError、CleanupError、ShutdownError
│   └── backend/
│       ├── unsupported.rs     # 非 Windows x86_64 失败后端
│       └── windows/           # 原生 SDK、设备、相机、帧与 callback 实现
├── mvs-sdk-sys/               # bindgen 生成的原始 C FFI crate
├── tests/                     # 公开契约、跨平台与真机 smoke tests
├── examples/                  # callback 与 polling 示例
└── tools/                     # Windows x64 bindings 生成脚本
```

分层关系：

```text
应用代码
  → mvs-sdk-rs 公共安全层
  → Windows backend / unsupported backend
  → mvs-sdk-sys
  → MvCameraControl 原生 SDK
```

- `src/lib.rs` 是平台无关的公开 API 边界，`src/backend/*` 必须保持私有。
- `mvs-sdk-sys` 只表达原始 C API，不承载安全抽象或业务状态；应用通常不直接依赖它。
- Windows backend 负责原生调用、状态机、callback trampoline 和 teardown；unsupported backend 保持接口同形但不伪造成功。
- 标准调用链为 `Sdk::init → enumerate_devices → DeviceInfo::open → callback/polling → Camera::close → Sdk::shutdown`。

## 官方依据与设计原则

本项目的 SDK 行为与安全设计必须以当前安装版本的官方资料为依据：

```text
C:\Program Files (x86)\MVS\Development\
├── Documentations\    # 官方开发文档
├── Includes\          # 官方头文件
└── Samples\           # 官方示例文件
```

- 开发前先查阅官方开发文档、头文件和示例，不得脱离官方资料自由推演 SDK 行为。
- API 签名、结构体、常量和返回码以头文件为准；调用语义和限制以开发文档为准；调用顺序、callback 用法和资源清理方式同时参考官方示例。
- 官方资料未直接说明的行为，应优先根据官方示例做最小、合理且可解释的推断，并把推断限制在解决当前问题所需的范围内。
- 安全设计必须有官方依据、可复现问题或测试证据。不要仅因理论风险新增全局状态、重复状态镜像、多层 wrapper、锁或复杂生命周期机制。
- 官方资料存在歧义时，保持结论克制，记录所依据的文档、头文件或示例，并用小范围测试验证；不要把推测写成确定契约。
- 在满足已确认契约和 Rust 安全要求的前提下，优先选择状态更少、层次更浅、容易审查的实现，保持代码简洁优雅。

## 核心安全约束

以下是当前源码与测试已经建立的约束；新增或强化约束仍需遵循上面的官方依据与最小设计原则。

- `Sdk::init()` 是进程级单例，成功后返回同一 `Arc<Sdk>`；初始化失败可以重试。
- `Sdk::shutdown()` 是显式终态操作：成功后不能重新初始化，Finalize 失败后状态为 unknown/poisoned。
- 存在正在打开或存活的 Camera、活动 callback、未确认销毁的 handle 时，禁止 shutdown。
- 设备枚举必须串行执行；枚举结果必须复制为 Rust-owned snapshot，不能借用厂商临时列表。
- `Camera` 是 `Send + !Sync`；同一实例的并发访问需要外部同步。
- callback 与 polling 模式互斥。首次注册、注销 image callback 或切换模式前必须停止采集。
- `Frame` 和 `EventInfo` 只在当前 callback/guard 借用窗口有效；跨线程或跨调用保存图像必须先 `Frame::to_owned()`。
- `FrameGuard` 是 `!Send + !Sync`，必须在取得它的线程使用；显式 `release()` 可报告错误，`Drop` 仅作兜底释放。
- callback 保持 `FnMut + Send + 'static`，允许 `!Sync` closure；SDK 线程上的 callback 应尽快返回。
- callback 地址必须稳定到原生 handle 确认销毁；同步重入必须被拒绝，closure 调用按 slot 串行化。
- closure、panic payload 和用户析构的 panic 都不能越过 `extern "C"` 边界；callback panic 后停用该 closure。
- 不要从同一 Camera 的 image/event callback 中 close 或 drop Camera；exception callback 仅使用厂商明确支持的 close/destroy 路径。
- 正常清理依次停止 callback 准入并 drain closure，再尽力执行 stop、注销 callback、CloseDevice 和 DestroyHandle；前一步失败不能阻止后续清理。
- `Camera::close()` 返回首个清理错误，`close_detailed()` 保留全部错误。`Drop` 只能 best effort，正常路径优先显式关闭。
- DestroyHandle 失败时必须保留 handle 和 callback backing，并登记 orphan；不能为避免泄漏而制造 use-after-free。
- event notification 与 Rust event closure 是独立状态；关闭通知不等于注销 closure。
- `as_raw()`/`as_raw_handle()` 只借出指针，不得借此关闭 handle 或绕过封装修改采集、callback 和生命周期状态。

## 编码规范

- 使用 Rust 2024 和默认 rustfmt；命名遵循 `PascalCase`、`snake_case`、`SCREAMING_SNAKE_CASE`。
- 按当前领域模块组织代码，默认使用最小可见性；公共 API 仅在 `src/lib.rs` 有意暴露。
- 平台无关语义放在公共层；target-specific `cfg`、raw C 类型与厂商布局放在 backend/sys 层。
- 原始 FFI 只出现在 `mvs-sdk-sys` 或 Windows backend；每个手写 `unsafe`/`unsafe impl` 必须有紧邻的 `SAFETY:` 说明。
- 所有 FFI 返回值都要处理；使用 `MvsResult`/`MvsError` 传播错误，并保留未知原始错误码。
- `unwrap()`/`expect()` 仅用于已局部证明的内部不变量；用户输入、环境、锁和 SDK 结果必须显式处理。
- 公开 rustdoc 与模块文档使用英文，README 和协作文档使用中文；文档重点说明生命周期、线程、错误和调用顺序。
- 修改公开 API 时同步检查 crate 根重导出、Windows/unsupported 后端、rustdoc、README 与 `tests/public_api.rs`。
- 单元测试贴近对应模块；公开所有权和 auto-trait 契约放在 `tests/public_api.rs`；真机测试保持 feature-gated、ignored、单线程运行。
- 不手工编辑 `mvs-sdk-sys/src/bindings.rs`；使用 `tools/generate-bindings-windows-x64.ps1` 重新生成。
- 修改保持聚焦，不整理任务范围外的代码；架构、安全约束或验证方式变化时同步更新本文档。

## 构建与验证

普通改动至少执行：

```console
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
```

涉及平台、feature 或公开 API 时补充：

```console
cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
cargo check --workspace --all-targets --features hardware-tests
```

真机测试仅在 Windows x64、MVS SDK、专用相机和 `MVS_TEST_CAMERA_SERIAL` 均已准备好时显式运行：

```console
cargo test --features hardware-tests --test hardware_smoke -- --ignored --test-threads=1
```

未运行的硬件或环境相关验证必须如实说明。

## Git 工作流

- 主分支为 `main`。
- 开始修改前检查 `git status`，保留用户已有改动；每个 commit 只处理一个清晰主题。
- 要有分点的正文。
- 提交标题采用 Conventional Commits 风格：

```text
<type>(<optional-scope>): <中文摘要>
```

常用 type：

- `feat`：新增能力
- `fix`：修复错误或安全问题
- `refactor`：不改变外部行为的结构调整
- `docs`：仅文档
- `test`：仅测试
- `chore`：仓库维护
- `build`：构建、链接或依赖配置

要求：

- 一个提交聚焦一个可审查主题，不混入无关重构。
- breaking change 使用 `!`，并在正文说明影响和迁移方式。
- FFI 安全、资源泄漏权衡或平台兼容改动应在正文说明原因、不变量和验证结果。

完成任务后的回复中，提供一个与实际 diff 对应的建议中文 commit message；不要声称已经 commit，除非确实执行且成功。
