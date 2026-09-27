//! 进程级 SDK 会话：初始化、设备枚举与打开相机。

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::error::sdk_call;
use crate::{AccessMode, Camera, DeviceInfo, Error, Result, TransportLayer, sys};

/// 本进程的 SDK 会话登记。
///
/// `None` 表示尚未成功 `MV_CC_Initialize`；能 upgrade 表示会话存活；不能 upgrade 表示已经（或正在）
/// `MV_CC_Finalize`。只在 Initialize 成功后写入，因此失败可以重试；厂商约定每个进程只初始化一次，
/// Finalize 之后不再重新初始化。
static SESSION: Mutex<Option<Weak<Session>>> = Mutex::new(None);

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
        // SAFETY: strong 计数已归零：所有相机均已销毁，SESSION 中的 Weak 也不会再 upgrade 成功，
        // 之后没有 SDK 调用；Finalize 只在此处调用。
        unsafe { sys::MV_CC_Finalize() };
    }
}

/// MVS SDK 的进程级入口。
///
/// 本进程只有一个会话，会话存活期间 [`Sdk::new`] 与 `clone` 得到的都是它。[`Camera`] 持有会话引用
/// 而不借用 `Sdk`，因此可以存入结构体或移动到其它线程；`Sdk` 与全部相机都释放后 SDK 自动反初始化。
/// 以 `&self` 借用 `Sdk` 的方法保证调用时 SDK 已初始化。`Sdk` 是 `Send + Sync`。
#[derive(Clone)]
#[must_use = "the SDK is finalized once the last Sdk and camera are dropped"]
pub struct Sdk {
    session: Arc<Session>,
}

impl Sdk {
    /// 返回本进程的 SDK 会话，首次调用时 `MV_CC_Initialize`。
    ///
    /// Initialize 失败返回 [`Error::Sdk`]，之后可以重试；会话 Finalize 之后返回 [`Error::Finalized`]。
    pub fn new() -> Result<Self> {
        let mut state = SESSION.lock().unwrap_or_else(PoisonError::into_inner);
        let session = match state.as_ref().map(Weak::upgrade) {
            Some(Some(session)) => session,
            Some(None) => return Err(Error::Finalized),
            None => {
                // SAFETY: 持锁且本进程尚未成功初始化，Initialize 不会并发或重复成功。
                unsafe { sdk_call!(MV_CC_Initialize()) }?;
                let session = Arc::new(Session {
                    enumeration: Mutex::new(()),
                });
                *state = Some(Arc::downgrade(&session));
                session
            }
        };
        Ok(Self { session })
    }

    /// 查询 SDK 版本，无需先初始化。
    pub fn version() -> u32 {
        // SAFETY: 厂商允许在 Initialize 之前调用，函数没有参数。
        unsafe { sys::MV_CC_GetSDKVersion() }
    }

    /// 枚举指定 transport 上的设备，返回的记录独立于 SDK 内部列表。
    pub fn devices(&self, layers: TransportLayer) -> Result<Vec<DeviceInfo>> {
        let _enumeration = self
            .session
            .enumeration
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut list = sys::MV_CC_DEVICE_INFO_LIST::default();
        // SAFETY: list 是可写输出。
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

impl fmt::Debug for Sdk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sdk").finish_non_exhaustive()
    }
}
