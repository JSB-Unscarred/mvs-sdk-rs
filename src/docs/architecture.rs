//! # 设计
//!
//! 本 crate 用所有权与借用表达 MVS SDK 的调用约定：每种 native 资源由一个类型拥有，在 `Drop` 中释放；
//! 错误的调用顺序无法通过编译。
//!
//! ## 所有权
//!
//! | 类型 | 拥有的资源 | 释放时 |
//! | --- | --- | --- |
//! | [`Sdk`](crate::Sdk) | 一份会话引用 | 最后一份引用释放时 `MV_CC_Finalize` |
//! | [`Camera`](crate::Camera) | 相机 handle、exception/event 闭包、一份会话引用 | `MV_CC_CloseDevice`、`MV_CC_DestroyHandle`，再释放闭包 |
//! | [`Grabbing`](crate::Grabbing) | 主动取图的取流状态 | `MV_CC_StopGrabbing` |
//! | [`CallbackGrabbing`](crate::CallbackGrabbing) | callback 取流状态与 image 闭包 | `MV_CC_StopGrabbing`，注销 callback，再释放闭包 |
//! | [`FrameGuard`](crate::FrameGuard) | 一个取到的 buffer | `MV_CC_FreeImageBuffer` |
//!
//! ## 会话
//!
//! 一个进程只有一个 SDK 会话，[`Sdk`](crate::Sdk) 与每台 [`Camera`](crate::Camera) 各持有一份引用：
//!
//! - 第一次调用 [`Sdk::new`](crate::Sdk::new) 时初始化 SDK，失败后可以重试；
//! - 会话存活期间，`Sdk::new` 与 `clone` 得到同一会话；
//! - 最后一份引用释放时 SDK 反初始化。厂商要求每个进程只初始化一次，此后 `Sdk::new` 返回
//!   [`Error::Finalized`](crate::Error::Finalized)。
//!
//! 相机因此不借用 `Sdk`，可以放进结构体或移到其它线程。需要反复打开、关闭相机的程序（例如含多个测试的
//! 测试程序）应始终持有一份 `Sdk`，以免会话提前结束。
//!
//! ## 取流
//!
//! [`Camera::start_grabbing`](crate::Camera::start_grabbing) 与
//! [`Camera::start_grabbing_with`](crate::Camera::start_grabbing_with) 返回的守卫可变借用相机，因此：
//!
//! - 取流期间不能再次开始取流、注册 callback 或关闭相机；
//! - 只有 [`Grabbing`](crate::Grabbing) 能主动取图，取到的 buffer 一定在停止取流前归还。
//!
//! 节点读写只需要 `&Camera`，取流期间经守卫的 `Deref` 调用。
//!
//! ## 清理失败
//!
//! `Drop` 忽略清理错误。需要检查时调用 [`Camera::close`](crate::Camera::close)、
//! [`Grabbing::stop`](crate::Grabbing::stop) 或 [`CallbackGrabbing::stop`](crate::CallbackGrabbing::stop)，
//! 它们执行完所有清理步骤，返回第一个错误。
//!
//! 清理失败后 SDK 可能仍会回调，相关闭包因此不释放：
//!
//! - 停止取流或注销失败时，image 闭包转交相机，保留到 `MV_CC_DestroyHandle`；
//! - `MV_CC_DestroyHandle` 失败时，闭包与一份会话引用被泄漏，本进程不再反初始化 SDK。
//!
//! ## Callback
//!
//! SDK 在自己的线程中调用 callback，闭包必须是 `Fn + Send + Sync + 'static`。
//!
//! - [`Frame`](crate::Frame) 与 [`EventInfo`](crate::EventInfo) 只在本次回调中有效，需要保留的数据要复制出来；
//! - exception/event 闭包保留到相机关闭，重复注册会累积闭包；
//! - callback 中的 panic 会终止进程；
//! - 不要在 callback 中关闭相机或停止取流，SDK 可能阻塞或报错；应通过 channel 交给持有相机的线程。
//!
//! ## 线程
//!
//! - [`Sdk`](crate::Sdk) 与 [`DeviceInfo`](crate::DeviceInfo) 是 `Send + Sync`；
//! - [`Camera`](crate::Camera) 是 `Send` 但不是 `Sync`，同一台相机的调用由持有它的线程发起；
//! - [`FrameGuard`](crate::FrameGuard) 不能跨线程。
//!
//! ## 字符串
//!
//! 字符串参数使用 `&CStr`，例如 `c"ExposureTime"`。SDK 返回的字符串不保证是 UTF-8：设备信息以 `&CStr`
//! 借出，[`Camera::get_string`](crate::Camera::get_string) 返回 `CString`。
//!
//! ## 错误
//!
//! SDK 调用失败时返回 [`Error::Sdk`](crate::Error::Sdk)，其中有失败的 SDK 函数名与
//! [`ErrorCode`](crate::ErrorCode)；各状态码的含义以厂商文档为准。
