//! SDK 初始化、反初始化与设备发现。

#[cfg(any(
    test,
    all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend;
use crate::camera::Camera;
use crate::device::DeviceInfo;
use crate::error::ShutdownError;
use crate::{AccessMode, MvsError, MvsResult, TransportLayer};

#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
static INITIALIZE_CLAIMED: AtomicBool = AtomicBool::new(false);

/// 声明一次进程级 Initialize 机会，成功后不复位。
#[cfg(any(
    test,
    all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
))]
fn claim_initialization(claimed: &AtomicBool) -> MvsResult<()> {
    if claimed.swap(true, Ordering::AcqRel) {
        Err(MvsError::InvalidState(
            "SDK initialization has already been attempted in this process",
        ))
    } else {
        Ok(())
    }
}

/// 由 `Sdk` 与已打开相机共享的一次性 native session。
///
/// `Arc` 只表达 session lease；相机 handle 仍由对应 `Camera` 唯一拥有。
/// `live_native_handles` 记录 CreateHandle 已写出、DestroyHandle 尚未确认的 handle。
pub(crate) struct RuntimeCore {
    inner: backend::Sdk,
    enumeration_lock: Mutex<()>,
    live_native_handles: AtomicUsize,
}

impl RuntimeCore {
    fn new(inner: backend::Sdk) -> Self {
        Self {
            inner,
            enumeration_lock: Mutex::new(()),
            live_native_handles: AtomicUsize::new(0),
        }
    }

    /// 记录 CreateHandle 已写出的非空 handle。
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    pub(crate) fn native_handle_created(&self) {
        self.live_native_handles.fetch_add(1, Ordering::AcqRel);
    }

    /// 记录 DestroyHandle 已确认销毁的 handle。
    ///
    /// 计数由 `NativeHandle` 的创建与销毁成对维护，因此下溢是内部 bug；
    /// release 下的绕回会让计数非零并继续阻止 Finalize，方向偏保守。
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    pub(crate) fn native_handle_destroyed(&self) {
        let previous = self.live_native_handles.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "native handle count underflowed");
    }

    fn orphaned_native_handles_live(&self) -> bool {
        self.live_native_handles.load(Ordering::Acquire) != 0
    }
}

/// 进程级 MVS SDK session 的唯一显式 Finalize 入口。
///
/// `Camera` 内部持有同一 session 的 lease，因此不借用本值。Initialize 每个进程
/// 最多尝试一次；普通 Drop 跳过 Finalize，正常路径应在其它 session owner 释放后调用
/// [`Sdk::shutdown`]；相机尚未关闭时该方法归还本值供重试。
pub struct Sdk {
    runtime: Arc<RuntimeCore>,
}

impl Sdk {
    /// 初始化进程级 SDK 资源。
    ///
    /// # Errors
    ///
    /// 支持的 native 进程重复调用时返回 [`MvsError::InvalidState`]；Initialize 失败也会
    /// 消费本进程唯一的一次尝试。
    pub fn initialize() -> MvsResult<Self> {
        Self::initialize_platform()
    }

    /// unsupported backend 不执行 native 初始化，也不消费进程级尝试。
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
    fn initialize_platform() -> MvsResult<Self> {
        backend::Sdk::init().map(|inner| Self {
            runtime: Arc::new(RuntimeCore::new(inner)),
        })
    }

    /// Windows x86_64 MSVC backend 串行声明唯一一次 native Initialize。
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    fn initialize_platform() -> MvsResult<Self> {
        claim_initialization(&INITIALIZE_CLAIMED)?;

        let inner = backend::Sdk::init()?;
        Ok(Self {
            runtime: Arc::new(RuntimeCore::new(inner)),
        })
    }

    /// 无需初始化即可查询已安装 SDK 的版本。
    pub fn version() -> MvsResult<u32> {
        backend::Sdk::sdk_version()
    }

    /// 枚举设备并返回 Rust-owned snapshot。
    ///
    /// 枚举锁只覆盖厂商内部列表的生成与复制，返回的设备信息不持有 session lease。
    pub fn devices(&self, layers: TransportLayer) -> MvsResult<Vec<DeviceInfo>> {
        let _enumeration = self
            .runtime
            .enumeration_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        backend::enumerate_devices(layers)
            .map(|devices| devices.into_iter().map(DeviceInfo::from_backend).collect())
    }

    /// 查询 owned 设备 snapshot 是否可按指定权限打开。
    pub fn is_accessible(&self, device: &DeviceInfo, mode: AccessMode) -> bool {
        device.is_accessible(mode)
    }

    /// 从 owned 设备 snapshot 创建并打开相机。
    ///
    /// key 仅对 native GigE 设备有意义；其它 transport 由 SDK 忽略。
    pub fn open(
        &self,
        device: &DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> MvsResult<Camera> {
        Camera::open(
            Arc::clone(&self.runtime),
            device.clone_backend(),
            mode,
            switchover_key,
        )
    }

    /// 消费唯一入口并反初始化 SDK。
    ///
    /// 其它 session owner 存在是可恢复情形：本方法归还 `Sdk`，调用方关闭相机后可重试，
    /// 通过 [`ShutdownError::into_sdk`] 取回。owner 已消费但 `DestroyHandle` 未确认成功
    /// 返回 [`MvsError::NativeHandlesLive`]，Finalize 失败返回原 native 错误；这两种
    /// 情形本进程的 Finalize 机会已消费，不归还 `Sdk`，调用方应按终止进程处理。
    pub fn shutdown(self) -> Result<(), ShutdownError> {
        // 本方法按值持有 `Sdk`，期间不存在 `&Sdk` 可再 clone lease；并发 Drop 的 Camera
        // 只会让计数下降，因此计数检查偏保守且无竞争。
        if Arc::strong_count(&self.runtime) != 1 {
            return Err(ShutdownError::recoverable(
                self,
                MvsError::InvalidState("all cameras must be dropped before SDK shutdown"),
            ));
        }

        // 上一步已确认本值是唯一 owner。
        let runtime = Arc::into_inner(self.runtime).expect("sole session owner");

        // 相机全部关闭后仍有 live handle，说明 DestroyHandle 未确认成功。
        if runtime.orphaned_native_handles_live() {
            return Err(ShutdownError::terminal(MvsError::NativeHandlesLive));
        }

        runtime.inner.finalize().map_err(ShutdownError::terminal)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use crate::MvsError;

    use super::claim_initialization;

    /// 验证进程级 Initialize claim 只允许一次成功声明。
    #[test]
    fn initialization_claim_is_one_shot() {
        let claimed = AtomicBool::new(false);
        assert!(claim_initialization(&claimed).is_ok());
        assert!(matches!(
            claim_initialization(&claimed),
            Err(MvsError::InvalidState(_))
        ));
    }
}
