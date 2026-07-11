//! Raw FFI bindings for the Hikvision MVS machine-vision camera SDK.
//!
//! The bindings are generated with `bindgen` and intentionally expose the
//! SDK's unsafe C API without a safety wrapper. Most applications should use
//! the safe `mvs-sdk-rs` crate instead.

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    unused_imports,
    clippy::all
)]

include!("bindings.rs");
