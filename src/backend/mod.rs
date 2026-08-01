//! Private platform implementation selected at compile time.

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CameraState {
    Open,
    Grabbing,
    Faulted,
    Closed,
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
impl CameraState {
    fn after_result<E>(result: &Result<(), E>, success: Self) -> Self {
        if result.is_ok() {
            success
        } else {
            Self::Faulted
        }
    }

    fn allows_normal_operations(self) -> bool {
        matches!(self, Self::Open | Self::Grabbing)
    }

    fn is_grabbing(self) -> bool {
        matches!(self, Self::Grabbing)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Grabbing => "Grabbing",
            Self::Faulted => "Faulted",
            Self::Closed => "Closed",
        }
    }
}

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
mod unsupported;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod windows;

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub(crate) use unsupported::*;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) use windows::*;

#[cfg(test)]
mod tests {
    use super::CameraState;

    #[test]
    fn camera_state_tracks_successful_start_and_stop() {
        let ok: Result<(), ()> = Ok(());

        let state = CameraState::after_result(&ok, CameraState::Grabbing);
        assert_eq!(state, CameraState::Grabbing);
        assert!(state.allows_normal_operations());
        assert!(state.is_grabbing());

        let state = CameraState::after_result(&ok, CameraState::Open);
        assert_eq!(state, CameraState::Open);
        assert!(state.allows_normal_operations());
        assert!(!state.is_grabbing());
    }

    #[test]
    fn uncertain_failure_faults_and_blocks_normal_operations() {
        let error: Result<(), ()> = Err(());
        let state = CameraState::after_result(&error, CameraState::Grabbing);

        assert_eq!(state, CameraState::Faulted);
        assert!(!state.allows_normal_operations());
        assert!(!CameraState::Closed.allows_normal_operations());
        assert_eq!(CameraState::Closed.name(), "Closed");
    }
}
