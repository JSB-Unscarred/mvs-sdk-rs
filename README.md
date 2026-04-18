# mvs_wrapper

海康威视 **MVS** 机器视觉相机 SDK 的安全 Rust 封装。所有 `unsafe` 均封在 crate 内部。

仅支持 **Windows x86_64 / x86**；其它平台上 crate 编译为空壳（无公开导出），`cargo check` 可正常通过。

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
use mvs_wrapper::{Library, TransportLayer};

let lib = Library::init()?;
let devs = lib.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
let mut cam = devs.iter().next().unwrap().open_exclusive()?;

cam.set_enum("TriggerMode", "Off")?;
cam.set_float("ExposureTime", 5000.0)?;
cam.register_image_callback(|f| println!("{}x{}", f.info().width(), f.info().height()))?;
cam.start_grabbing()?;
```

## 维护者：重新生成 bindings

`src/bindings.rs` 已提交到仓库，普通使用者**不需要** libclang。SDK 升级后重新生成：

```cmd
cargo build --features bindgen
```

需要 LLVM（`scoop install llvm` 或设 `LIBCLANG_PATH`）。

## License

MIT OR Apache-2.0
