//! Private platform implementation selected at compile time.

#[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
mod unsupported;
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
mod windows;

#[cfg(not(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")))]
pub(crate) use unsupported::*;
#[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
pub(crate) use windows::*;
