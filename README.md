# mvs_sdk_rs

海康威视机器人 **MVS** 工业相机 SDK 的安全 Rust 封装。workspace 包含面向应用的
`mvs-sdk-rs` 和承载原始 FFI 的 `mvs-sdk-sys`；业务代码通常只需依赖前者。

真实相机访问目前仅支持 **Windows x86_64**。其它目标提供相同的公开 API，但
`Sdk::init` 会返回 `MvsError::UnsupportedPlatform`。Windows 构建与链接需要：

- 已安装 MVS SDK；
- `MVCAM_COMMON_RUNENV` 指向 SDK 的 `Development` 目录。

运行应用时，MVS DLL 所在目录还必须能由 Windows loader 找到，通常将其加入
`PATH`。

## 回调取流

常用流程是初始化 SDK、枚举并打开相机、配置节点、注册图像回调，然后开始采集：

```rust
use std::{
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use mvs_sdk_rs::{Sdk, TransportLayer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = Sdk::init()?;
    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let Some(device) = devices.iter().next() else {
        println!("No camera found");
        sdk.shutdown()?;
        return Ok(());
    };

    println!("Open {} {} SN={}", device.manufacturer(), device.model(), device.serial());
    let mut camera = device.open_exclusive()?;
    camera.set_enum("TriggerMode", "Off")?;
    camera.set_float("ExposureTime", 5000.0)?;

    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    camera.register_image_callback(move |frame| {
        let info = frame.info();
        let _ = frame_tx.try_send((
            info.frame_num(), info.width(), info.height(), frame.data().len()
        ));
    })?;

    camera.start_grabbing()?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((number, width, height, bytes)) => {
                println!("frame={number} size={width}x{height} bytes={bytes}");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    camera.stop_grabbing()?;
    camera.close()?;
    sdk.shutdown()?;
    Ok(())
}
```

完整版本见 [`examples/callback.rs`](examples/callback.rs)。

## 轮询取图

未注册图像回调时，`start_grabbing` 进入轮询模式。`FrameGuard` 借用 SDK buffer；
显式 `release` 可以观察释放错误，直接 drop 则执行无法报告错误的兜底释放。

```rust
camera.start_grabbing()?;

let guard = camera.get_image_buffer(1000)?;
let frame = guard.frame();
println!("{:?}", frame.info());
let owned = frame.to_owned();
guard.release()?;

println!("copied {} bytes", owned.data().len());
camera.stop_grabbing()?;
camera.close()?;
sdk.shutdown()?;
```

完整版本见 [`examples/polling.rs`](examples/polling.rs)。
运行示例时需启用 `hardware-tests` feature，例如
`cargo run --features hardware-tests --example callback`。

## 生命周期与安全语义

- 图像 callback 模式与轮询模式互斥。首次注册、注销或切换模式前需停止采集；callback 模式采集中可直接替换已注册的 Rust closure，不会再次调用 SDK。
- callback 由 SDK 线程调用，应尽快返回。首次 panic 会被截获并停用该 closure，重新注册后恢复。`Frame` 和 `EventInfo` 只在当前调用中有效；跨线程或跨调用保存图像请先 `to_owned`。
- 可同时持有的 `FrameGuard` 数量受 SDK 图像节点数限制。guard 存活时相机保持借用，不能停止或关闭。
- 正常路径优先显式 `Camera::close`；它返回首个清理错误，`close_detailed` 可取得完整报告，`Drop` 只做忽略错误的兜底清理。
- 不要从同一相机的 image/event callback 内调用 `close`。断连 exception callback 中官方示例使用的 close/destroy 子序列见 `Camera::close_detailed` rustdoc。
- `event_notification_off` 只关闭设备端通知；移除 Rust closure 仍需 `unregister_event_callback`。
- `Camera` 是 `Send` 但不是 `Sync`；同一实例的并发访问需要外部同步。`FrameGuard` 既不是 `Send` 也不是 `Sync`。
- `as_raw_handle` 只借出由 `Camera` 拥有的指针。通过它调用 FFI 属于 unsafe 高级用法，不得绕过封装关闭 handle 或改变采集、回调状态。
- `Sdk::shutdown` 只能在相机已关闭且 callback 已退出后调用；成功 shutdown 后，同一进程不能再次初始化 SDK。
- `ShutdownError::InUse` 可在资源释放后重试；无法确认 handle 销毁或 Finalize 失败时，应记录错误并结束进程。

## API 文档

公开类型、方法、错误、节点值和像素格式以 rustdoc 为准，不在 README 重复维护：

```console
cargo doc --workspace --no-deps --open
```

发布版本也可查看 [docs.rs](https://docs.rs/mvs-sdk-rs)。原始 C API 位于
`mvs-sdk_sys`；除非需要 safe crate 尚未覆盖的厂商能力，否则不建议直接依赖。

## 测试

不链接真实 SDK、也不需要相机的静态检查。最后一条会显式编译 feature-gated examples
和 hardware smoke，但不会运行或链接它们：

```console
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo check --workspace --all-targets --target x86_64-unknown-linux-gnu
cargo check --workspace --all-targets --features hardware-tests
```

普通测试不访问相机，也不会构建需要真实 SDK 的 examples：

```console
cargo test --workspace
```

真实 SDK 与专用相机 smoke test 是一条串行终态工作流：枚举、节点访问、双 buffer、
callback、`Camera::close`、`Sdk::shutdown`。它需要 Windows x64、MVS SDK、正确的
`MVCAM_COMMON_RUNENV` 和 DLL `PATH`、环境变量 `MVS_TEST_CAMERA_SERIAL`，并要求该
专用相机的 `TriggerMode=Off`：

```console
cargo test --features hardware-tests --test hardware_smoke -- --ignored --test-threads=1
```

## 维护 bindings

`mvs-sdk-sys/src/bindings.rs` 已提交，普通构建不需要 libclang。升级 Windows x64
MVS SDK 后，在安装 LLVM/libclang 与 `bindgen-cli` 的环境运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

脚本默认从 `MVCAM_COMMON_RUNENV` 查找 SDK，也可通过 `-SdkRoot` 指定开发目录，并要求
`bindgen-cli 0.72.1`（安装命令：`cargo install bindgen-cli --version 0.72.1 --locked`）。
bindings 生成不会在普通 Cargo 构建中自动发生。

## License

MIT
