//! SDK lifetime, process-wide initialization, and live-resource tracking.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

use crate::backend;
use crate::device::DeviceList;
use crate::{MvsError, MvsResult, TransportLayer};

static PROCESS: OnceLock<ProcessRuntime> = OnceLock::new();

/// Return the process-wide SDK runtime state.
fn process() -> &'static ProcessRuntime {
    PROCESS.get_or_init(ProcessRuntime::new)
}

/// Own the process-wide state and resource counters.
struct ProcessRuntime {
    state: RwLock<ProcessState>,
    resources: Arc<ResourceLedger>,
}

impl ProcessRuntime {
    /// Construct the runtime before the first native initialization.
    fn new() -> Self {
        Self {
            state: RwLock::new(ProcessState::Uninitialized),
            resources: Arc::new(ResourceLedger::default()),
        }
    }
}

/// Track the only three public SDK lifecycle states.
enum ProcessState {
    Uninitialized,
    Active(Arc<Sdk>),
    Finalized,
}

/// Process-wide handle to the initialized MVS SDK.
///
/// Every successful call to [`Sdk::init`] returns an [`Arc`] to the same
/// allocation. Native lifetime is controlled explicitly by [`Sdk::shutdown`],
/// not by the last `Arc` being dropped.
pub struct Sdk {
    pub(crate) inner: backend::Sdk,
    sdk_version: u32,
    enumeration_lock: Mutex<()>,
    resources: Arc<ResourceLedger>,
}

impl Sdk {
    /// Initialize the process-wide MVS SDK runtime.
    ///
    /// Successful calls return clones of the same [`Arc`]. A failed native
    /// initialization leaves the runtime uninitialized, so a later call may
    /// retry after the environment has been repaired.
    ///
    /// # Errors
    ///
    /// Returns [`MvsError::UnsupportedPlatform`] outside Windows x86_64 and
    /// [`MvsError::SdkFinalized`] after successful [`Sdk::shutdown`].
    pub fn init() -> MvsResult<Arc<Self>> {
        let runtime = process();
        let mut state = runtime
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match &*state {
            ProcessState::Active(sdk) => return Ok(Arc::clone(sdk)),
            ProcessState::Finalized => return Err(MvsError::SdkFinalized),
            ProcessState::Uninitialized => {}
        }

        let inner = backend::Sdk::init()?;
        let sdk_version = inner.sdk_version();
        let sdk = Arc::new(Self {
            inner,
            sdk_version,
            enumeration_lock: Mutex::new(()),
            resources: Arc::clone(&runtime.resources),
        });
        *state = ProcessState::Active(Arc::clone(&sdk));
        Ok(sdk)
    }

    /// Finalize the process-wide MVS SDK.
    ///
    /// Every camera must be dropped and every callback must have returned.
    /// A native handle that could not be destroyed also keeps the SDK in use.
    /// A successful shutdown is idempotent and terminal for this process. A
    /// native finalization failure leaves the SDK active so the caller may
    /// retry.
    ///
    /// # Errors
    ///
    /// Returns [`MvsError::SdkInUse`] while a camera, unresolved handle, or
    /// callback is live, or the native finalization error.
    pub fn shutdown(&self) -> MvsResult<()> {
        let runtime = process();
        let mut state = runtime
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match &*state {
            ProcessState::Finalized => return Ok(()),
            ProcessState::Uninitialized => return Err(MvsError::SdkNotInitialized),
            ProcessState::Active(_) => {}
        }

        if self.resources.is_in_use() {
            return Err(MvsError::SdkInUse);
        }

        self.inner.finalize()?;
        *state = ProcessState::Finalized;
        Ok(())
    }

    /// Return the SDK version cached during initialization.
    pub fn sdk_version(&self) -> u32 {
        self.sdk_version
    }

    /// Enumerate connected devices of the requested transport types.
    ///
    /// The SDK reuses internal enumeration storage. Serializing enumeration
    /// keeps that storage valid until every returned record has been copied.
    pub fn enumerate_devices(&self, layers: TransportLayer) -> MvsResult<DeviceList> {
        let _active = self.operation()?;
        let _enumeration = self
            .enumeration_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        DeviceList::enumerate(layers)
    }

    /// Hold the process lifecycle read lock for one operation on this SDK.
    pub(crate) fn operation(&self) -> MvsResult<ActiveSdk> {
        ActiveSdk::acquire()
    }

    /// Hold the process lifecycle read lock for an operation without an SDK receiver.
    pub(crate) fn active() -> MvsResult<ActiveSdk> {
        ActiveSdk::acquire()
    }
}

/// Read guard proving that the process-wide SDK is active.
pub(crate) struct ActiveSdk {
    state: RwLockReadGuard<'static, ProcessState>,
}

impl ActiveSdk {
    /// Acquire an active-runtime guard so shutdown cannot race the operation.
    fn acquire() -> MvsResult<Self> {
        let state = process()
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            ProcessState::Active(_) => Ok(Self { state }),
            ProcessState::Uninitialized => Err(MvsError::SdkNotInitialized),
            ProcessState::Finalized => Err(MvsError::SdkFinalized),
        }
    }

    /// Return the SDK protected by this guard.
    fn sdk(&self) -> &Sdk {
        let ProcessState::Active(sdk) = &*self.state else {
            unreachable!("ActiveSdk is constructed only for the active state")
        };
        sdk
    }

    /// Count one successfully opened camera until its public owner is dropped.
    pub(crate) fn camera_lease(&self) -> CameraLease {
        self.sdk().resources.camera_lease()
    }
}

/// Count resources that must finish before process-wide finalization.
#[derive(Default)]
pub(crate) struct ResourceLedger {
    live_cameras: AtomicUsize,
    active_callbacks: AtomicUsize,
}

impl ResourceLedger {
    /// Return whether native work still depends on the initialized SDK.
    fn is_in_use(&self) -> bool {
        self.live_cameras.load(Ordering::Acquire) != 0
            || self.active_callbacks.load(Ordering::Acquire) != 0
    }

    /// Count one opened camera; the returned lease removes the count on drop.
    fn camera_lease(self: &Arc<Self>) -> CameraLease {
        self.retain_camera();
        CameraLease {
            ledger: Arc::clone(self),
        }
    }

    /// Retain one native camera dependency until its owner can prove destruction.
    fn retain_camera(&self) {
        self.live_cameras.fetch_add(1, Ordering::AcqRel);
    }

    /// Count one native callback invocation until its trampoline returns.
    #[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
    fn enter_callback(&self) -> CallbackGuard<'_> {
        self.active_callbacks.fetch_add(1, Ordering::AcqRel);
        CallbackGuard { ledger: self }
    }
}

/// Count a callback invocation in the process-wide resource ledger.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn enter_callback() -> CallbackGuard<'static> {
    process().resources.enter_callback()
}

/// Keep finalization blocked after the SDK refuses to destroy a native handle.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn retain_unresolved_handle() {
    process().resources.retain_camera();
}

/// Live-camera count owned by one public [`Camera`](crate::Camera).
pub(crate) struct CameraLease {
    ledger: Arc<ResourceLedger>,
}

impl Drop for CameraLease {
    /// Remove the count when the public Camera owner is consumed or dropped.
    fn drop(&mut self) {
        let previous = self.ledger.live_cameras.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "camera count underflowed");
    }
}

/// Active-callback count owned by one trampoline invocation.
#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
pub(crate) struct CallbackGuard<'a> {
    ledger: &'a ResourceLedger,
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
impl Drop for CallbackGuard<'_> {
    /// Remove the count after the callback returns through every path.
    fn drop(&mut self) {
        let previous = self.ledger.active_callbacks.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "callback count underflowed");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ResourceLedger;

    // 验证 camera、callback 与未销毁 handle 共同控制 SDK 的 in-use 条件。
    #[test]
    fn resource_leases_block_shutdown_until_drop() {
        let ledger = Arc::new(ResourceLedger::default());
        assert!(!ledger.is_in_use());

        let camera = ledger.camera_lease();
        let callback = ledger.enter_callback();
        assert!(ledger.is_in_use());

        drop(camera);
        assert!(ledger.is_in_use());
        drop(callback);
        assert!(!ledger.is_in_use());

        ledger.retain_camera();
        assert!(ledger.is_in_use());
    }
}
