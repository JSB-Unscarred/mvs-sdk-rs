//! Build script for mvs_wrapper.
//!
//! Responsibilities:
//!   1. Refuse non-Windows / non-x86-family targets at build time.
//!   2. Locate the MVS SDK via `MVCAM_COMMON_RUNENV` and emit link directives.
//!   3. Optionally (with `--features bindgen`) regenerate `src/bindings.rs`.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MVCAM_COMMON_RUNENV");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_os != "windows" {
        println!(
            "cargo:warning=mvs_wrapper only supports Windows; skipping MVS SDK link configuration."
        );
        return;
    }
    if target_arch != "x86_64" {
        println!(
            "cargo:warning=mvs_wrapper only supports x86_64 on Windows; skipping MVS SDK link configuration."
        );
        return;
    }

    println!("cargo:rustc-cfg=mvs_platform");

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

    configure_link(&mvcam, &target_arch);

    #[cfg(feature = "bindgen")]
    regenerate_bindings(&mvcam);
}

fn configure_link(mvcam: &Path, _target_arch: &str) {
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

#[cfg(feature = "bindgen")]
fn regenerate_bindings(mvcam: &Path) {
    let include_path = mvcam.join("Includes");
    let header = include_path.join("MvCameraControl.h");

    if !header.exists() {
        panic!(
            "MvCameraControl.h not found at {}. Is the SDK installed correctly?",
            header.display()
        );
    }

    println!("cargo:rerun-if-changed={}", header.display());

    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_path.display()))
        // Include only the active SDK surface; exclude obsolete interfaces.
        .allowlist_function("MV_CC_.*")
        .allowlist_function("MV_GIGE_.*")
        .allowlist_function("MV_USB_.*")
        .allowlist_function("MV_CAML_.*")
        .allowlist_function("MV_GENTL_.*")
        .allowlist_function("MV_XML_.*")
        .allowlist_function("MV_SetLogPath")
        .allowlist_function("MV_SetLogLevel")
        .allowlist_type("MV_.*")
        .allowlist_type("_MV_.*")
        .allowlist_type("Mv.*")
        .allowlist_type("_Mv.*")
        .allowlist_var("MV_.*")
        .allowlist_var("INFO_MAX_BUFFER_SIZE")
        .allowlist_var("MAX_EVENT_NAME_SIZE")
        .allowlist_var("MAX_STRING_.*")
        .allowlist_var("PIXEL_.*")
        .blocklist_file(".*MvObsoleteInterfaces\\.h")
        .blocklist_file(".*ObsoleteCamParams\\.h")
        .derive_default(true)
        .derive_debug(true)
        .derive_copy(true)
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed to generate MVS bindings");

    let out_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("src")
        .join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write src/bindings.rs");
    println!(
        "cargo:warning=Regenerated bindings at {}",
        out_path.display()
    );
}
