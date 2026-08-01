//! Private platform implementation selected at compile time.

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionMode {
    Callback,
    Polling,
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
impl AcquisitionMode {
    fn name(self) -> &'static str {
        match self {
            Self::Callback => "Callback",
            Self::Polling => "Polling",
        }
    }
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CameraState {
    Open,
    Grabbing(AcquisitionMode),
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
        matches!(self, Self::Open | Self::Grabbing(_))
    }

    fn is_grabbing(self) -> bool {
        matches!(self, Self::Grabbing(_))
    }

    fn acquisition_mode(self) -> Option<&'static str> {
        match self {
            Self::Grabbing(mode) => Some(mode.name()),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Grabbing(_) => "Grabbing",
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
    use super::{AcquisitionMode, CameraState};

    #[test]
    fn camera_state_tracks_both_grabbing_modes_and_successful_stop() {
        let ok: Result<(), ()> = Ok(());

        for (mode, expected_name) in [
            (AcquisitionMode::Callback, "Callback"),
            (AcquisitionMode::Polling, "Polling"),
        ] {
            let state = CameraState::Grabbing(mode);
            assert!(state.allows_normal_operations());
            assert!(state.is_grabbing());
            assert_eq!(state.acquisition_mode(), Some(expected_name));
            assert_eq!(state.name(), "Grabbing");
        }

        let state = CameraState::after_result(&ok, CameraState::Open);
        assert_eq!(state, CameraState::Open);
        assert!(state.allows_normal_operations());
        assert!(!state.is_grabbing());
        assert_eq!(state.acquisition_mode(), None);
    }

    #[test]
    fn uncertain_failure_faults_and_blocks_normal_operations() {
        let error: Result<(), ()> = Err(());
        let state = CameraState::after_result(&error, CameraState::Open);

        assert_eq!(state, CameraState::Faulted);
        assert!(!state.allows_normal_operations());
        assert!(!CameraState::Closed.allows_normal_operations());
        assert_eq!(CameraState::Closed.name(), "Closed");
    }
}
