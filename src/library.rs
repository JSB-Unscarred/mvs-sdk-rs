//! SDK lifetime, process-wide initialization, and native-resource tracking.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

use crate::backend;
use crate::device::DeviceList;
use crate::{MvsError, MvsResult, ShutdownError, TransportLayer};

static PROCESS: OnceLock<ProcessRuntime> = OnceLock::new();

fn process() -> &'static ProcessRuntime {
    PROCESS.get_or_init(ProcessRuntime::new)
}

struct ProcessRuntime {
    state: RwLock<ProcessState>,
    resources: Arc<ResourceLedger>,
}

impl ProcessRuntime {
    fn new() -> Self {
        Self {
            state: RwLock::new(ProcessState::Uninitialized),
            resources: Arc::new(ResourceLedger::default()),
        }
    }
}

enum ProcessState {
    Uninitialized,
    Active(Arc<Sdk>),
    Finalized,
    Poisoned { finalize_code: Option<u32> },
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
    /// initialization is not cached, so a later call may retry after the
    /// runtime environment has been repaired.
    ///
    /// # Errors
    ///
    /// Returns [`MvsError::UnsupportedPlatform`] outside Windows x86_64.
    /// Initialization also fails after a successful [`Sdk::shutdown`] or when
    /// a failed finalization left the process-wide SDK state unknown.
    pub fn init() -> MvsResult<Arc<Self>> {
        Self::init_with(process(), || {
            let inner = backend::Sdk::init()?;
            let sdk_version = inner.sdk_version();
            Ok((inner, sdk_version))
        })
    }

    fn init_with(
        runtime: &ProcessRuntime,
        initialize: impl FnOnce() -> MvsResult<(backend::Sdk, u32)>,
    ) -> MvsResult<Arc<Self>> {
        let mut state = runtime
            .state
            .write()
            .map_err(|_| MvsError::SdkStateUnknown)?;

        match &*state {
            ProcessState::Active(sdk) => return Ok(Arc::clone(sdk)),
            ProcessState::Finalized => return Err(MvsError::SdkFinalized),
            ProcessState::Poisoned { .. } => return Err(MvsError::SdkStateUnknown),
            ProcessState::Uninitialized => {}
        }

        let resources = Arc::clone(&runtime.resources);
        let (inner, sdk_version) = initialize()?;
        let sdk = Arc::new(Self {
            inner,
            sdk_version,
            enumeration_lock: Mutex::new(()),
            resources,
        });
        *state = ProcessState::Active(Arc::clone(&sdk));
        Ok(sdk)
    }

    /// Finalize the process-wide MVS SDK.
    ///
    /// This is a terminal operation: successful shutdown cannot be followed
    /// by another initialization in the same process. The call is idempotent,
    /// but it refuses to finalize while a camera or callback remains live, or
    /// after native handle destruction could not be confirmed. Close every
    /// [`Camera`](crate::Camera), wait for callbacks to return, and then call
    /// this method if explicit process-wide finalization is required.
    ///
    /// # Errors
    ///
    /// See [`ShutdownError`] for live resources, unresolved handles, native
    /// finalization failures, and an already-unknown process state.
    pub fn shutdown(&self) -> Result<(), ShutdownError> {
        self.shutdown_with(process(), || self.inner.finalize())
    }

    fn shutdown_with(
        &self,
        runtime: &ProcessRuntime,
        finalize: impl FnOnce() -> MvsResult<()>,
    ) -> Result<(), ShutdownError> {
        let mut state = runtime
            .state
            .write()
            .map_err(|_| ShutdownError::StateUnknown {
                finalize_code: None,
            })?;

        match &*state {
            ProcessState::Finalized => return Ok(()),
            ProcessState::Poisoned { finalize_code } => {
                return Err(ShutdownError::StateUnknown {
                    finalize_code: *finalize_code,
                });
            }
            ProcessState::Uninitialized => {
                return Err(ShutdownError::StateUnknown {
                    finalize_code: None,
                });
            }
            ProcessState::Active(active) if !std::ptr::eq(self, Arc::as_ptr(active)) => {
                return Err(ShutdownError::StateUnknown {
                    finalize_code: None,
                });
            }
            ProcessState::Active(_) => {}
        }

        let resources = self.resources.snapshot();
        if resources.orphaned_handles != 0 {
            return Err(ShutdownError::UnresolvedResources {
                orphaned_handles: resources.orphaned_handles,
            });
        }
        if resources.live_cameras != 0 || resources.active_callbacks != 0 {
            return Err(ShutdownError::InUse {
                live_cameras: resources.live_cameras,
                active_callbacks: resources.active_callbacks,
            });
        }

        match finalize() {
            Ok(()) => {
                *state = ProcessState::Finalized;
                Ok(())
            }
            Err(error) => {
                let finalize_code = error.raw_code();
                *state = ProcessState::Poisoned { finalize_code };
                Err(ShutdownError::Finalize(error))
            }
        }
    }

    /// SDK version as a packed `u32`; interpret per MVS SDK documentation.
    /// The value is cached during initialization and remains readable after
    /// shutdown without entering the native SDK again.
    pub fn sdk_version(&self) -> u32 {
        self.sdk_version
    }

    /// Enumerate connected devices of the requested transport types.
    ///
    /// Combine layers with `|`, for example
    /// `TransportLayer::GIGE | TransportLayer::USB`. Returned device metadata
    /// is copied into Rust-owned snapshots and remains inspectable after a
    /// later enumeration.
    ///
    /// # Errors
    ///
    /// Returns an SDK lifecycle error after shutdown, or the vendor error from
    /// device enumeration.
    pub fn enumerate_devices(&self, layers: TransportLayer) -> MvsResult<DeviceList> {
        let _operation = self.operation()?;
        let _enumeration = self
            .enumeration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        DeviceList::enumerate(layers)
    }

    pub(crate) fn operation(&self) -> MvsResult<OperationPermit> {
        let active = ActiveSdk::acquire()?;
        if !std::ptr::eq(self, active.sdk.as_ref()) {
            return Err(MvsError::SdkFinalized);
        }
        Ok(OperationPermit {
            _state: active.state,
        })
    }

    pub(crate) fn active() -> MvsResult<ActiveSdk> {
        ActiveSdk::acquire()
    }
}

pub(crate) struct ActiveSdk {
    sdk: Arc<Sdk>,
    state: RwLockReadGuard<'static, ProcessState>,
}

impl ActiveSdk {
    fn acquire() -> MvsResult<Self> {
        let state = process()
            .state
            .read()
            .map_err(|_| MvsError::SdkStateUnknown)?;
        let sdk = match &*state {
            ProcessState::Active(sdk) => Arc::clone(sdk),
            ProcessState::Uninitialized => return Err(MvsError::SdkNotInitialized),
            ProcessState::Finalized => return Err(MvsError::SdkFinalized),
            ProcessState::Poisoned { .. } => return Err(MvsError::SdkStateUnknown),
        };
        Ok(Self { sdk, state })
    }

    pub(crate) fn begin_camera_open(&self) -> PendingCameraLease {
        PendingCameraLease::new(Arc::clone(&self.sdk.resources))
    }
}

/// Keeps the process lifecycle in `Active` while a short global FFI call is
/// in progress.
pub(crate) struct OperationPermit {
    _state: RwLockReadGuard<'static, ProcessState>,
}

#[derive(Default)]
pub(crate) struct ResourceLedger {
    counts: Mutex<ResourceCounts>,
    active_callbacks: AtomicUsize,
}

#[derive(Default)]
struct ResourceCounts {
    live_cameras: usize,
    orphaned_handles: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResourceSnapshot {
    pub(crate) live_cameras: usize,
    pub(crate) active_callbacks: usize,
    pub(crate) orphaned_handles: usize,
}

impl ResourceLedger {
    fn with_counts<T>(&self, f: impl FnOnce(&mut ResourceCounts) -> T) -> T {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut counts)
    }

    fn snapshot(&self) -> ResourceSnapshot {
        self.with_counts(|counts| ResourceSnapshot {
            live_cameras: counts.live_cameras,
            active_callbacks: self.active_callbacks.load(Ordering::Acquire),
            orphaned_handles: counts.orphaned_handles,
        })
    }

    #[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
    fn enter_callback(self: &Arc<Self>) -> CallbackGuard {
        self.active_callbacks.fetch_add(1, Ordering::AcqRel);
        CallbackGuard {
            ledger: Arc::clone(self),
        }
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn enter_callback() -> CallbackGuard {
    process().resources.enter_callback()
}

pub(crate) struct PendingCameraLease {
    ledger: Option<Arc<ResourceLedger>>,
}

impl PendingCameraLease {
    fn new(ledger: Arc<ResourceLedger>) -> Self {
        Self {
            ledger: Some(ledger),
        }
    }

    pub(crate) fn opened(mut self) -> CameraLease {
        let ledger = self.ledger.take().expect("pending lease already settled");
        ledger.with_counts(|counts| counts.live_cameras += 1);
        CameraLease {
            ledger: Some(ledger),
        }
    }

    pub(crate) fn failed(mut self, orphaned: bool) {
        let ledger = self.ledger.take().expect("pending lease already settled");
        if orphaned {
            ledger.with_counts(|counts| counts.orphaned_handles += 1);
        }
    }
}

impl Drop for PendingCameraLease {
    fn drop(&mut self) {
        if let Some(ledger) = self.ledger.take() {
            ledger.with_counts(|counts| counts.orphaned_handles += 1);
        }
    }
}

pub(crate) struct CameraLease {
    ledger: Option<Arc<ResourceLedger>>,
}

impl CameraLease {
    pub(crate) fn settle(&mut self, destroyed: bool) {
        if let Some(ledger) = self.ledger.take() {
            ledger.with_counts(|counts| {
                counts.live_cameras = counts.live_cameras.saturating_sub(1);
                if !destroyed {
                    counts.orphaned_handles += 1;
                }
            });
        }
    }
}

impl Drop for CameraLease {
    fn drop(&mut self) {
        self.settle(false);
    }
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
pub(crate) struct CallbackGuard {
    ledger: Arc<ResourceLedger>,
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
impl Drop for CallbackGuard {
    fn drop(&mut self) {
        let previous = self.ledger.active_callbacks.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "callback activity count underflowed");
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessRuntime, ResourceLedger, Sdk};
    use crate::{MvsError, ShutdownError, backend, sys};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn camera_and_callback_leases_update_one_shared_ledger() {
        let ledger = Arc::new(ResourceLedger::default());
        let pending = super::PendingCameraLease::new(Arc::clone(&ledger));
        assert_eq!(ledger.snapshot(), Default::default());

        let mut camera = pending.opened();
        let callback = ledger.enter_callback();
        assert_eq!(ledger.snapshot().live_cameras, 1);
        assert_eq!(ledger.snapshot().active_callbacks, 1);

        camera.settle(true);
        drop(callback);
        assert_eq!(ledger.snapshot(), Default::default());
    }

    #[test]
    fn unresolved_camera_becomes_an_orphan() {
        let ledger = Arc::new(ResourceLedger::default());
        let camera = super::PendingCameraLease::new(Arc::clone(&ledger)).opened();
        drop(camera);
        assert_eq!(ledger.snapshot().orphaned_handles, 1);
    }

    #[test]
    fn abandoned_camera_open_becomes_an_orphan() {
        let ledger = Arc::new(ResourceLedger::default());
        let pending = super::PendingCameraLease::new(Arc::clone(&ledger));
        drop(pending);
        assert_eq!(ledger.snapshot().orphaned_handles, 1);
    }

    #[test]
    fn initialization_caches_only_success() {
        let runtime = ProcessRuntime::new();
        let first = Sdk::init_with(&runtime, || Err(MvsError::Resource));
        assert!(matches!(first, Err(MvsError::Resource)));

        let sdk = Sdk::init_with(&runtime, || Ok((backend::Sdk::test_instance(), 0x01020304)))
            .expect("second initialization succeeds");
        let same = Sdk::init_with(&runtime, || panic!("successful init must be cached"))
            .expect("cached SDK is returned");

        assert!(Arc::ptr_eq(&sdk, &same));
        assert_eq!(sdk.sdk_version(), 0x01020304);
    }

    #[test]
    fn live_camera_blocks_shutdown_then_shutdown_is_idempotent() {
        let runtime = ProcessRuntime::new();
        let sdk = Sdk::init_with(&runtime, || Ok((backend::Sdk::test_instance(), 1))).unwrap();
        let mut camera = super::PendingCameraLease::new(Arc::clone(&sdk.resources)).opened();
        let callback = sdk.resources.enter_callback();
        let finalizations = AtomicUsize::new(0);

        let blocked = sdk.shutdown_with(&runtime, || {
            finalizations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(matches!(blocked, Err(ShutdownError::InUse { .. })));
        assert_eq!(finalizations.load(Ordering::SeqCst), 0);

        camera.settle(true);
        let blocked = sdk.shutdown_with(&runtime, || {
            finalizations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(matches!(
            blocked,
            Err(ShutdownError::InUse {
                live_cameras: 0,
                active_callbacks: 1
            })
        ));
        drop(callback);

        sdk.shutdown_with(&runtime, || {
            finalizations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        sdk.shutdown_with(&runtime, || panic!("Finalize must not be called twice"))
            .unwrap();
        assert_eq!(finalizations.load(Ordering::SeqCst), 1);
        assert!(matches!(
            Sdk::init_with(&runtime, || panic!(
                "finalized SDK must not initialize again"
            )),
            Err(MvsError::SdkFinalized)
        ));
    }

    #[test]
    fn finalize_failure_poisoning_is_terminal() {
        let runtime = ProcessRuntime::new();
        let sdk = Sdk::init_with(&runtime, || Ok((backend::Sdk::test_instance(), 1))).unwrap();

        let failed = sdk.shutdown_with(&runtime, || Err(MvsError::Resource));
        assert!(matches!(
            failed,
            Err(ShutdownError::Finalize(MvsError::Resource))
        ));
        assert!(matches!(
            sdk.shutdown_with(&runtime, || panic!("Finalize failure must not be retried")),
            Err(ShutdownError::StateUnknown {
                finalize_code: Some(code)
            }) if code == sys::MV_E_RESOURCE
        ));
        assert!(matches!(
            Sdk::init_with(&runtime, || panic!("poisoned runtime cannot initialize")),
            Err(MvsError::SdkStateUnknown)
        ));
    }
}
