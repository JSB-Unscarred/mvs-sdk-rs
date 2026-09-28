//! # 设计
//!
//! 本 crate 用所有权与借用表达 MVS SDK 的调用约定：每种 native 资源由一个 Rust 类型拥有并在 `Drop`
//! 中释放，错误的调用顺序在编译期就无法写出。
//!
//! ## 所有权
//!
//! | 类型 | 拥有的资源 | 释放时 |
//! | --- | --- | --- |
//! | [`Sdk`](crate::Sdk) | 一份 SDK 会话 | 最后一份会话释放时 `MV_CC_Finalize` |
//! | [`Camera`](crate::Camera) | 相机 handle、exception/event 闭包、一份会话 | `MV_CC_CloseDevice`、`MV_CC_DestroyHandle`，再释放闭包 |
//! | [`Grabbing`](crate::Grabbing) | 主动取图的取流状态 | `MV_CC_StopGrabbing` |
//! | [`CallbackGrabbing`](crate::CallbackGrabbing) | callback 取流状态与 image 闭包 | `MV_CC_StopGrabbing`，注销 callback，再释放闭包 |
//! | [`FrameGuard`](crate::FrameGuard) | 一个取到的 buffer | `MV_CC_FreeImageBuffer` |
//!
//! ## 会话
//!
//! 一个进程只有一个 SDK 会话，[`Sdk`](crate::Sdk) 与每个 [`Camera`](crate::Camera) 各持有一份：
//!
//! - 第一次调用 [`Sdk::new`](crate::Sdk::new) 时初始化 SDK，失败后可以重试；
//! - 会话存活期间，`Sdk::new` 与 `clone` 得到同一会话；
//! - `Sdk` 与所有相机都释放后 SDK 反初始化。厂商要求每个进程只初始化一次，之后 `Sdk::new` 返回
//!   [`Error::Finalized`](crate::Error::Finalized)。
//!
//! 相机不借用 `Sdk`，可以放进结构体或移到其它线程。程序若先后多次创建并释放全部 `Sdk` 与相机
//! （例如同一测试程序中的多个测试），应保留一份 `Sdk`，否则后面的调用会得到 `Finalized`。
//!
//! ## 取流
//!
//! [`Camera::start_grabbing`](crate::Camera::start_grabbing) 与
//! [`Camera::start_grabbing_with`](crate::Camera::start_grabbing_with) 返回的守卫可变借用相机，因此：
//!
//! - 只有 [`Grabbing`](crate::Grabbing) 能主动取图，callback 模式没有这个方法；
//! - 取流期间不能再次开始取流、注册 callback 或关闭相机；
//! - 取到的 buffer 与守卫都会在相机关闭前释放。
//!
//! 节点读写只需要 `&Camera`，取流期间经守卫的 `Deref` 仍可调用。
//!
//! ## 清理失败
//!
//! `Drop` 忽略清理错误；需要检查时调用 [`Camera::close`](crate::Camera::close)、
//! [`Grabbing::stop`](crate::Grabbing::stop) 或 [`CallbackGrabbing::stop`](crate::CallbackGrabbing::stop)，
//! 它们会执行完所有清理步骤并返回第一个错误。
//!
//! 清理失败时 SDK 可能仍会调用已注册的闭包，因此闭包不会被释放：停止取流或注销失败时，image 闭包
//! 交给相机保留到 `MV_CC_DestroyHandle`；`MV_CC_DestroyHandle` 失败时闭包与会话都会泄漏，本进程不再
//! 反初始化 SDK。
//!
//! ## Callback
//!
//! SDK 在自己的线程中调用 callback，所以闭包必须是 `Fn + Send + Sync + 'static`。
//!
//! - [`Frame`](crate::Frame) 与 [`EventInfo`](crate::EventInfo) 只在本次回调中有效，需要保留的数据要复制出来；
//! - exception/event 闭包保留到相机关闭，重复注册会累积闭包；
//! - callback 中的 panic 会终止进程；
//! - 不要在 callback 中关闭相机或停止取流，SDK 可能阻塞或报错。应通过 channel 交给持有相机的线程处理。
//!
//! ## 线程
//!
//! [`Sdk`](crate::Sdk) 与 [`DeviceInfo`](crate::DeviceInfo) 是 `Send + Sync`；
//! [`Camera`](crate::Camera) 是 `Send` 但不是 `Sync`，同一台相机的调用需要由持有它的线程发起；
//! [`FrameGuard`](crate::FrameGuard) 不能跨线程。
//!
//! ## 字符串
//!
//! 字符串参数使用 `&CStr`，例如 `c"ExposureTime"`。设备信息等 SDK 字符串以 `&CStr` 返回，
//! [`Camera::get_string`](crate::Camera::get_string) 返回拥有的 `CString`。
//!
//! ## 错误
//!
//! SDK 调用失败时返回 [`Error::Sdk`](crate::Error::Sdk)，其中包含失败的 SDK 函数名与
//! [`ErrorCode`](crate::ErrorCode)，各状态码的含义以厂商文档为准。
