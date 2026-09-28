//! # 设计原则
//!
//! 本 crate 把 MVS C API 的调用约定映射为 Rust 的所有权、借用与类型，而不是运行时状态检查。
//! 结构与 [realsense-rust](https://gitlab.com/tangram-vision/oss/realsense-rust) 一致，并与姊妹 crate
//! [`mv3d-lp`](https://crates.io/crates/mv3d-lp)（海康机器人 3D 激光轮廓传感器）保持对称：
//!
//! 1. `mvs-sdk-sys` 只包含 bindgen 生成的声明与 `raw-dylib` 链接属性，只支持 `x86_64-pc-windows-msvc`。
//! 2. 每种 native 资源对应一个拥有它的 Rust 类型，`Drop` 负责释放；方法直接调用 `sys`。
//! 3. 非法调用顺序在类型上不可表达。
//! 4. 优先使用 Rust 原生类型：`&CStr`、`CString`、`Option<Duration>`、`Ipv4Addr`。
//!
//! ## 所有权
//!
//! | 类型 | 拥有的资源 | 释放 |
//! | --- | --- | --- |
//! | [`Sdk`](crate::Sdk) | 一份 SDK 会话引用 | 最后一份引用释放时 `MV_CC_Finalize` |
//! | [`Camera`](crate::Camera) | handle、exception/event 闭包的强引用与事件名、一份会话引用 | `CloseDevice` → `DestroyHandle` → 释放闭包引用 |
//! | [`Grabbing`](crate::Grabbing) | 取流状态（可变借用相机） | `StopGrabbing` |
//! | [`CallbackGrabbing`](crate::CallbackGrabbing) | 取流状态与 image 闭包的强引用 | `StopGrabbing` → 注销 → 释放闭包引用 |
//! | [`FrameGuard`](crate::FrameGuard) | 一个 pull buffer（借用 `Grabbing`） | `FreeImageBuffer` |
//!
//! ## 会话
//!
//! 本进程只有一个 SDK 会话，由 [`Sdk`](crate::Sdk) 与每个 [`Camera`](crate::Camera) 以 `Arc` 共享，
//! 并以 `Weak` 登记在进程级 static 中：
//!
//! - 首次调用 [`Sdk::new`](crate::Sdk::new) 时 `MV_CC_Initialize`，成功后才登记，因此失败可以重试；
//! - 会话存活期间 `Sdk::new` 与 `clone` 得到同一会话，丢掉 `Sdk` 后只要还有相机存活，仍能取回；
//! - 最后一份引用释放时 `MV_CC_Finalize`。厂商约定每个进程只初始化一次，之后 `Sdk::new` 返回
//!   [`Error::Finalized`](crate::Error::Finalized)。
//!
//! 相机不借用 `Sdk`，可以存入结构体或移到工作线程。同一测试二进制中的多个真机测试应共享一份
//! 保活的 `Sdk`，否则前一个测试释放会话后，后一个测试会得到 `Finalized`。
//!
//! ## 类型状态
//!
//! 取流守卫可变借用相机，借用检查器因此保证：
//!
//! - pull 取图只能在 [`Grabbing`](crate::Grabbing) 上调用，callback 模式没有这个方法；
//! - 取流期间不能再次开始取流、注册 callback 或关闭相机；
//! - buffer 与取流守卫都在相机关闭前释放。
//!
//! 节点读写只需要 `&Camera`，取流期间经 `Deref` 仍可调用。守卫、相机与 `Sdk` 都标记了 `#[must_use]`，
//! `camera.start_grabbing()?;` 这类立即丢弃守卫的写法会得到编译警告。
//!
//! ## 失败与清理
//!
//! `Drop` 忽略清理错误；需要观察时调用 [`Camera::close`](crate::Camera::close)、
//! [`Grabbing::stop`](crate::Grabbing::stop) 或 [`CallbackGrabbing::stop`](crate::CallbackGrabbing::stop)，
//! 它们都会执行完所有清理步骤并返回首个错误。
//!
//! 清理失败时 SDK 可能仍持有闭包指针，因此：
//!
//! - `StopGrabbing` 或 image callback 注销失败：闭包的强引用转交相机，保留到 `DestroyHandle`；
//! - `DestroyHandle` 失败：泄漏闭包的强引用与一份会话引用，闭包永不释放，本进程不再执行 Finalize。
//!
//! 这些保证只依赖所有权与 `Arc` 计数。
//!
//! ## Callback
//!
//! 注册时把闭包放入 `Arc`，以 `Arc::into_raw` 的地址作为 `pUser`，trampoline 以具体类型 `F` 还原闭包，
//! 不经过锁或全局表。相机或取流守卫持有注册时的强引用，trampoline 在每次调用期间再持有一份：
//! callback 中释放相机或守卫不会释放正在执行的闭包，最后一份引用可能在 SDK 线程释放。
//! 闭包必须是 `Fn + Send + Sync + 'static`，因为 SDK 在内部线程调用它。
//!
//! - 参数一律按值传入：[`Frame`](crate::Frame)、[`EventInfo`](crate::EventInfo) 是只在本次回调期间有效的
//!   借用视图，[`ExceptionKind`](crate::ExceptionKind) 是普通值；
//! - image callback 使用 `MV_CC_RegisterImageCallBackEx2(bAutoFree = true)`；
//! - exception/event 闭包可能随时被调用，保留到 `DestroyHandle`，重复注册会累积闭包；
//! - panic 越过 `extern "C"` 时进程终止（Rust 1.81 起的语言行为）。
//!
//! 本 crate 假定 `StopGrabbing`、注销与 `DestroyHandle` 成功返回后 SDK 不再以该 `pUser` 回调。
//! 在 callback 中关闭相机或结束取流时，SDK 可能阻塞或返回错误，回调参数借用的 SDK 内存也只能依赖厂商
//! 保证；最后一个相机释放时还会在 SDK 线程执行 Finalize。因此应通过 channel 通知 owner 线程。
//!
//! ## 线程
//!
//! [`Sdk`](crate::Sdk) 与 [`DeviceInfo`](crate::DeviceInfo) 是 `Send + Sync`；
//! [`Camera`](crate::Camera) 是 `Send` 但不是 `Sync`，同一 handle 的调用由 owner 串行发起；
//! [`FrameGuard`](crate::FrameGuard) 不能跨线程。
//!
//! ## 字符串
//!
//! 字符串参数使用 `&CStr`，不存在 interior NUL 错误。SDK 定长字符数组中的字符串以 NUL 结尾，
//! 厂商示例直接以 `%s` 读取：借用型 getter 返回截到 NUL 的 `&CStr`，缺少 NUL 的违约数据读作空串；
//! [`Camera::get_string`](crate::Camera::get_string) 这类拥有型输出在字段写满时保留整个字段。
//!
//! ## 错误
//!
//! SDK 失败统一为 [`Error::Sdk`](crate::Error::Sdk)，携带失败的函数名与
//! [`ErrorCode`](crate::ErrorCode)；各方法不再逐一列出可能的状态码，含义以厂商文档为准。
//! 错误信息为英文，便于写入日志与检索。
