//! Private platform implementation selected at compile time.

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
mod unsupported;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod windows;

#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
pub(crate) use unsupported::*;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub(crate) use windows::*;
