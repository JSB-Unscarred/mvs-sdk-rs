//! Build script for mvs_sdk_sys.
//!
//! Responsibilities:
//!   1. Skip MVS SDK link configuration outside Windows x86_64.
//!   2. Locate the MVS SDK via `MVCAM_COMMON_RUNENV` and emit link directives.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MVCAM_COMMON_RUNENV");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_os != "windows" {
        return;
    }
    if target_arch != "x86_64" {
        println!(
            "cargo:warning=mvs_sdk_sys only supports x86_64 on Windows; skipping MVS SDK link configuration."
        );
        return;
    }

    let mvcam = match env::var("MVCAM_COMMON_RUNENV") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            println!(
                "cargo:warning=MVCAM_COMMON_RUNENV is not set. `cargo check` will still work, \
                 but linking requires the MVS SDK. Example: \
                 set MVCAM_COMMON_RUNENV=\"C:\\Program Files (x86)\\MVS\\Development\""
            );
            return;
        }
    };

    let lib_dir = mvcam.join("Libraries").join("win64");

    if !lib_dir.exists() {
        panic!(
            "MVS library directory does not exist: {}\n\
             Verify that the MVS SDK is installed at MVCAM_COMMON_RUNENV = {}.",
            lib_dir.display(),
            mvcam.display()
        );
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=MvCameraControl");
}
