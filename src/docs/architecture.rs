//! # 设计原则
//!
//! 本 crate 把 MVS C API 的调用约定映射为 Rust 的所有权、借用与类型，而不是运行时状态检查。
//! 结构与 [realsense-rust](https://gitlab.com/tangram-vision/oss/realsense-rust) 一致：
//!
//! 1. `mvs-sdk-sys` 只包含 bindgen 生成的声明和链接配置。
//! 2. 每种 native 资源对应一个拥有它的 Rust 类型，`Drop` 负责释放；方法直接调用 `sys`。
//! 3. 非法调用顺序在类型上不可表达。
//! 4. 优先使用 Rust 原生类型：`&CStr`、`CString`、`Option<Duration>`、`Ipv4Addr`。
//!
//! ## 所有权
//!
//! | 类型 | 拥有的资源 | 释放 |
//! | --- | --- | --- |
//! | [`Sdk`](crate::Sdk) | 一份 SDK 会话引用 | 最后一份引用释放时 `MV_CC_Finalize` |
//! | [`Camera`](crate::Camera) | handle、exception/event 闭包、一份会话引用 | `CloseDevice` → `DestroyHandle` → 释放闭包 |
//! | [`Grabbing`](crate::Grabbing) | 取流状态（可变借用相机） | `StopGrabbing` |
//! | [`CallbackGrabbing`](crate::CallbackGrabbing) | 取流状态与 image 闭包 | `StopGrabbing` → 注销 → 释放闭包 |
//! | [`FrameGuard`](crate::FrameGuard) | 一个 polling buffer（借用 `Grabbing`） | `FreeImageBuffer` |
//!
//! 会话用 `Arc` 共享，[`Camera`](crate::Camera) 不借用 [`Sdk`](crate::Sdk)，可以存入结构体或移到
//! 工作线程；只要还有相机存活，Finalize 就不会执行。厂商约定每个进程只初始化一次，
//! 会话存活期间 [`Sdk::new`](crate::Sdk::new) 返回同一会话，Finalize 之后返回
//! [`Error::Finalized`](crate::Error::Finalized)。
//!
//! ## 类型状态
//!
//! 取流守卫可变借用相机，借用检查器因此保证：
//!
//! - polling 取图只能在 [`Grabbing`](crate::Grabbing) 上调用，callback 模式没有这个方法；
//! - 取流期间不能再次开始取流、注册 callback 或关闭相机；
//! - buffer 与取流守卫都在相机关闭前释放。
//!
//! 节点读写只需要 `&Camera`，取流期间经 `Deref` 仍可调用。
//!
//! ## 失败与清理
//!
//! `Drop` 忽略清理错误；需要观察时调用 [`Camera::close`](crate::Camera::close)、
//! [`Grabbing::stop`](crate::Grabbing::stop) 或 [`CallbackGrabbing::stop`](crate::CallbackGrabbing::stop)，
//! 它们都会执行完所有清理步骤并返回首个错误。
//!
//! 清理失败时 SDK 可能仍持有闭包指针，因此：
//!
//! - `StopGrabbing` 或 image callback 注销失败：闭包转交相机，保留到 `DestroyHandle`；
//! - `DestroyHandle` 失败：泄漏闭包与一份会话引用，本进程不再执行 Finalize。
//!
//! 这些保证只依赖所有权，不需要计数器或全局状态。
//!
//! ## Callback
//!
//! 注册时把 `Box<F>` 的地址作为 `pUser`，trampoline 以具体类型 `F` 还原闭包，不经过锁或全局表。
//! 闭包必须是 `Fn + Send + Sync + 'static`，因为 SDK 在内部线程调用它。
//!
//! - image callback 使用 `MV_CC_RegisterImageCallBackEx2(bAutoFree = true)`，
//!   [`Frame`](crate::Frame) 只在本次回调期间有效；
//! - exception/event 闭包可能随时被调用，保留到 `DestroyHandle`，重复注册会累积闭包；
//! - panic 越过 `extern "C"` 时进程终止（Rust 1.81 起的语言行为）；
//! - 不要在 callback 中关闭相机或改变取流状态，应通过 channel 通知 owner 线程。
//!
//! ## 线程
//!
//! [`Sdk`](crate::Sdk) 与 [`DeviceInfo`](crate::DeviceInfo) 是 `Send + Sync`；
//! [`Camera`](crate::Camera) 是 `Send` 但不是 `Sync`，同一 handle 的调用由 owner 串行发起；
//! [`FrameGuard`](crate::FrameGuard) 不能跨线程。
//!
//! ## 错误
//!
//! SDK 失败统一为 [`Error::Sdk`](crate::Error::Sdk)，携带失败的函数名与
//! [`ErrorCode`](crate::ErrorCode)；各方法不再逐一列出可能的状态码，含义以厂商文档为准。
//! 字符串参数使用 `&CStr`，不存在 interior NUL 错误。
