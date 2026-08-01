//! Private platform implementation selected at compile time.

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionState {
    Stopped,
    Callback,
    Polling,
    Unknown,
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
impl AcquisitionState {
    fn requires_stop(self) -> bool {
        !matches!(self, Self::Stopped)
    }

    fn mode_name(self) -> Option<&'static str> {
        match self {
            Self::Callback => Some("Callback"),
            Self::Polling => Some("Polling"),
            Self::Stopped | Self::Unknown => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Callback => "Callback",
            Self::Polling => "Polling",
            Self::Unknown => "Unknown",
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
    use super::AcquisitionState;

    #[test]
    fn known_grabbing_states_expose_their_modes_and_require_stop() {
        for (state, expected_name) in [
            (AcquisitionState::Callback, "Callback"),
            (AcquisitionState::Polling, "Polling"),
        ] {
            assert!(state.requires_stop());
            assert_eq!(state.mode_name(), Some(expected_name));
            assert_eq!(state.name(), expected_name);
        }
    }

    #[test]
    fn stopped_requires_no_cleanup_stop_and_has_no_mode() {
        let state = AcquisitionState::Stopped;

        assert!(!state.requires_stop());
        assert_eq!(state.mode_name(), None);
        assert_eq!(state.name(), "Stopped");
    }

    #[test]
    fn unknown_is_not_confirmed_grabbing_but_requires_a_recovery_stop() {
        let state = AcquisitionState::Unknown;

        assert!(state.requires_stop());
        assert_eq!(state.mode_name(), None);
        assert_eq!(state.name(), "Unknown");
    }
}
