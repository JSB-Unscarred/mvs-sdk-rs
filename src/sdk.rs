//! 进程级 SDK 会话：初始化、设备枚举与打开相机。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::sdk_call;
use crate::{AccessMode, Camera, DeviceInfo, Error, Result, TransportLayer, sys};

/// 本进程是否已尝试 `MV_CC_Initialize`；厂商约定每个进程只初始化一次。
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 已初始化的 SDK 会话，由 [`Sdk`] 与每个 [`Camera`] 通过 `Arc` 共享。
///
/// 最后一个持有者释放时调用 `MV_CC_Finalize`，因此相机不会比会话活得更久。
/// 相机的 `DestroyHandle` 失败时会泄漏一份引用，使 Finalize 不再执行。
pub(crate) struct Session {
    /// `MV_CC_EnumDevices` 输出的指针指向 SDK 内部列表，下一次枚举会覆盖它，复制完成前需串行。
    enumeration: Mutex<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: 所有相机都持有会话引用，走到这里说明它们均已销毁；Finalize 只在此处调用。
        unsafe { sys::MV_CC_Finalize() };
    }
}

/// MVS SDK 的进程级入口。
///
/// [`Camera`] 持有同一会话的引用而不借用 `Sdk`，因此可以存入结构体或移动到其它线程；
/// `Sdk` 与全部相机都释放后 SDK 自动反初始化。`Sdk` 是 `Send + Sync`。
pub struct Sdk {
    session: Arc<Session>,
}

impl Sdk {
    /// 初始化 SDK。每个进程只能成功调用一次，之后返回 [`Error::AlreadyInitialized`]。
    pub fn initialize() -> Result<Self> {
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            return Err(Error::AlreadyInitialized);
        }
        // SAFETY: 上面的原子标记保证本进程只调用一次。
        unsafe { sdk_call!(MV_CC_Initialize()) }?;
        Ok(Self { session: Arc::new(Session { enumeration: Mutex::new(()) }) })
    }

    /// 查询 SDK 版本，无需先初始化。
    pub fn version() -> u32 {
        // SAFETY: 厂商允许在 Initialize 之前调用，函数没有参数。
        unsafe { sys::MV_CC_GetSDKVersion() }
    }

    /// 枚举指定 transport 上的设备，返回的记录独立于 SDK 内部列表。
    pub fn devices(&self, layers: TransportLayer) -> Result<Vec<DeviceInfo>> {
        let _enumeration = self.session.enumeration.lock().unwrap_or_else(PoisonError::into_inner);
        let mut list = sys::MV_CC_DEVICE_INFO_LIST::default();
        // SAFETY: list 是可写的输出结构体。
        unsafe { sdk_call!(MV_CC_EnumDevices(layers.raw(), &raw mut list)) }?;

        let count = (list.nDeviceNum as usize).min(list.pDeviceInfo.len());
        let mut devices = Vec::with_capacity(count);
        for &info in &list.pDeviceInfo[..count] {
            // SAFETY: 非空项由本次枚举写入，枚举锁保证复制完成前列表不被覆盖。
            if let Some(info) = unsafe { info.as_ref() } {
                devices.push(DeviceInfo::from_raw(info));
            }
        }
        Ok(devices)
    }

    /// 查询设备当前能否以指定模式打开。
    #[allow(clippy::unused_self, reason = "借用 Sdk 保证调用时会话仍处于初始化状态")]
    pub fn is_accessible(&self, device: &DeviceInfo, mode: AccessMode) -> bool {
        let mut raw = *device.raw();
        // SAFETY: raw 是设备记录的本地副本，C 接口只是签名要求可变指针。
        unsafe { sys::MV_CC_IsDeviceAccessible(&raw mut raw, mode as u32) != 0 }
    }

    /// 创建 handle 并打开相机；`switchover_key` 只对原生 `GigE` 设备有意义。
    pub fn open(
        &self,
        device: &DeviceInfo,
        mode: AccessMode,
        switchover_key: u16,
    ) -> Result<Camera> {
        Camera::open(Arc::clone(&self.session), device, mode, switchover_key)
    }
}
