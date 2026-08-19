//! SDK 初始化、反初始化与设备发现。

#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
use std::sync::atomic::AtomicUsize;
#[cfg(any(
    test,
    all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend;
use crate::camera::Camera;
use crate::device::DeviceInfo;
use crate::{AccessMode, MvsError, MvsResult, TransportLayer};

#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
static INITIALIZE_CLAIMED: AtomicBool = AtomicBool::new(false);
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
static LIVE_NATIVE_HANDLES: AtomicUsize = AtomicUsize::new(0);

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

/// 记录 CreateHandle 已写出的非空 handle。
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
pub(crate) fn native_handle_created() {
    LIVE_NATIVE_HANDLES.fetch_add(1, Ordering::AcqRel);
}

/// 记录 DestroyHandle 已确认销毁的 handle。
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
pub(crate) fn native_handle_destroyed() {
    let previous = LIVE_NATIVE_HANDLES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        count.checked_sub(1)
    });
    debug_assert!(previous.is_ok(), "native handle count underflowed");
}

/// 返回 Rust owner 已消费但仍未确认销毁的 native handle 是否存在。
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
fn orphaned_native_handles_live() -> bool {
    LIVE_NATIVE_HANDLES.load(Ordering::Acquire) != 0
}

/// 由 `Sdk` 与已打开相机共享的一次性 native session。
///
/// `Arc` 只表达 session lease；相机 handle 仍由对应 `Camera` 唯一拥有。
pub(crate) struct RuntimeCore {
    inner: backend::Sdk,
    enumeration_lock: Mutex<()>,
}

/// 进程级 MVS SDK session 的唯一显式 Finalize 入口。
///
/// `Camera` 内部持有同一 session 的 lease，因此不借用本值。Initialize 每个进程
/// 最多尝试一次；普通 Drop 跳过 Finalize，正常路径应在其它 session owner 释放后调用
/// [`Sdk::shutdown`]。
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
            runtime: Arc::new(RuntimeCore {
                inner,
                enumeration_lock: Mutex::new(()),
            }),
        })
    }

    /// Windows x86_64 MSVC backend 串行声明唯一一次 native Initialize。
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    fn initialize_platform() -> MvsResult<Self> {
        claim_initialization(&INITIALIZE_CLAIMED)?;

        let inner = backend::Sdk::init()?;
        Ok(Self {
            runtime: Arc::new(RuntimeCore {
                inner,
                enumeration_lock: Mutex::new(()),
            }),
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
    /// 其它 session owner 存在时返回 [`MvsError::InvalidState`]。owner 已消费但
    /// `DestroyHandle` 未确认成功时返回 [`MvsError::NativeHandlesLive`]。两种错误均不
    /// 调用 Finalize，调用方应按终止进程处理；本方法消费 `Sdk`，不能重试。
    pub fn shutdown(self) -> MvsResult<()> {
        let runtime = Arc::try_unwrap(self.runtime).map_err(|_| {
            MvsError::InvalidState("all cameras must be dropped before SDK shutdown")
        })?;

        #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
        if orphaned_native_handles_live() {
            return Err(MvsError::NativeHandlesLive);
        }

        runtime.inner.finalize()
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
