//! SDK 初始化、反初始化与设备枚举。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::backend;
use crate::device::DeviceList;
use crate::{MvsError, MvsResult, TransportLayer};

const SDK_UNUSED: u8 = 0;
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
const SDK_ACTIVE: u8 = 1;
const SDK_TERMINATED: u8 = 2;

static SDK_STATE: AtomicU8 = AtomicU8::new(SDK_UNUSED);
static LIVE_NATIVE_HANDLES: AtomicUsize = AtomicUsize::new(0);

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

/// 返回进程内是否仍有 native handle 未确认销毁。
pub(crate) fn native_handles_live() -> bool {
    LIVE_NATIVE_HANDLES.load(Ordering::Acquire) != 0
}

/// 进程内唯一的 MVS SDK owner。
///
/// `DeviceList`、`DeviceInfo` 和 `Camera` 均借用该值，因此 Rust 会阻止相机
/// 资源存活时执行 [`Sdk::shutdown`]。官方文档限定单进程只调用一次 Initialize
/// 与 Finalize，成功初始化后的状态不会回到未初始化。
pub struct Sdk {
    inner: backend::Sdk,
    enumeration_lock: Mutex<()>,
    active: bool,
}

impl Sdk {
    /// 初始化进程级 SDK 资源。
    ///
    /// # Errors
    ///
    /// SDK owner 已存在时返回 [`MvsError::SdkInUse`]；反初始化已执行时返回
    /// [`MvsError::SdkTerminated`]。
    pub fn init() -> MvsResult<Self> {
        #[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
        {
            backend::Sdk::init().map(|inner| Self {
                inner,
                enumeration_lock: Mutex::new(()),
                active: true,
            })
        }

        #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
        {
            match SDK_STATE.compare_exchange(
                SDK_UNUSED,
                SDK_ACTIVE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {}
                Err(SDK_ACTIVE) => return Err(MvsError::SdkInUse),
                Err(SDK_TERMINATED) => return Err(MvsError::SdkTerminated),
                Err(_) => unreachable!("SDK state only uses declared constants"),
            }

            match backend::Sdk::init() {
                Ok(inner) => Ok(Self {
                    inner,
                    enumeration_lock: Mutex::new(()),
                    active: true,
                }),
                Err(error) => {
                    // 官方限定每进程只调用一次 Initialize；失败也不再重试。
                    SDK_STATE.store(SDK_TERMINATED, Ordering::Release);
                    Err(error)
                }
            }
        }
    }

    /// 无需初始化即可查询已安装 SDK 的版本。
    pub fn sdk_version() -> MvsResult<u32> {
        backend::Sdk::sdk_version()
    }

    /// 枚举设备并立即复制 SDK 管理的记录。
    ///
    /// 枚举锁只保护厂商会在下一次枚举时重建的内部列表，复制完成后即释放。
    pub fn enumerate_devices(&self, layers: TransportLayer) -> MvsResult<DeviceList<'_>> {
        let _enumeration = self
            .enumeration_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        DeviceList::enumerate(self, layers)
    }

    /// 消费唯一 owner 并反初始化 SDK。
    ///
    /// 借用该 owner 的设备和相机会在编译期阻止本调用。无论 native 返回值如何，
    /// Finalize 都只尝试一次；存在未确认销毁的 handle 时返回 [`MvsError::SdkInUse`]
    /// 并保留 active 状态。
    pub fn shutdown(mut self) -> MvsResult<()> {
        self.finalize()
    }

    fn finalize(&mut self) -> MvsResult<()> {
        if !self.active {
            return Ok(());
        }
        if native_handles_live() {
            return Err(MvsError::SdkInUse);
        }
        self.active = false;
        SDK_STATE.store(SDK_TERMINATED, Ordering::Release);
        self.inner.finalize()
    }
}

impl Drop for Sdk {
    /// 显式 shutdown 可报告错误；Drop 只负责本 owner 的兜底反初始化。
    fn drop(&mut self) {
        let _ = self.finalize();
    }
}
