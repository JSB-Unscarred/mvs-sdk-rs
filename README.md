# MVS-SDK-RS

海康威视机器人 **MVS** 工业相机 SDK 的安全 Rust 封装。所有 `unsafe` 均封在 crate 内部。

目前仅支持 **Windows x86_64**；其它目标使用 stub API，仅用于让 `cargo check` 正常通过。

## 使用

1. 安装 MVS SDK（<https://www.hikrobotics.com/>）
2. 安装器会自动设置 `MVCAM_COMMON_RUNENV` 并把运行时 DLL 加入 `PATH`
3. `Cargo.toml`（路径依赖或 git 依赖，按需选择）:

```toml
[dependencies]
# 路径依赖（本地开发 / git submodule）
mvs_wrapper = { path = "libs/mvs_wrapper" }

```

```rust
use mvs_wrapper::{Sdk, TransportLayer};

let sdk = Sdk::init()?;
let devs = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;

if let Some(dev) = devs.iter().next() {
    let mut cam = dev.open_exclusive()?;
    cam.set_enum("TriggerMode", "Off")?;
    cam.set_float("ExposureTime", 5000.0)?;
    cam.register_image_callback(|f| println!("{}x{}", f.info().width(), f.info().height()))?;
    cam.start_grabbing()?;
}
```

## 维护：重新生成 bindings

`src/bindings.rs` 已提交到仓库，普通使用者**不需要** libclang。

SDK 升级后重新生成：

```cmd
cargo build --features bindgen
```

运行上述命令需要安装 LLVM 并将其添加到环境变量中。

## License
