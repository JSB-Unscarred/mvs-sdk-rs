//! Private platform implementation selected at compile time.

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionState {
    Stopped,
    Callback,
    Polling,
    Unknown,
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
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
