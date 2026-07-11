# mvs-sdk-sys

Raw, unsafe FFI bindings for the Hikvision MVS machine-vision camera SDK.
The bindings are generated with `bindgen` and are primarily an implementation
dependency of the safe `mvs-sdk-rs` crate. Application code should normally
depend on `mvs-sdk-rs` instead.

The native MVS SDK is currently supported on Windows x86_64. The generated
bindings are committed, so ordinary builds do not require libclang.

From the workspace root, maintainers can regenerate the bindings with:

```text
cargo build -p mvs-sdk-sys --features bindgen
```

`MVCAM_COMMON_RUNENV` must point to an MVS SDK development directory containing
`Includes/MvCameraControl.h`.
