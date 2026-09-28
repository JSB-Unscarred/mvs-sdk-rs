# mvs-sdk-sys

Unofficial raw FFI bindings for the Hikrobot MVS industrial camera SDK (`MvCameraControl`).
Most applications should use the safe [`mvs-sdk`](https://crates.io/crates/mvs-sdk) crate, which re-exports
this crate as `mvs_sdk::sys`.

- Target: `x86_64-pc-windows-msvc` only.
- Building does not need the MVS SDK: the bindings are generated from MVS 4.7.0 headers and link
  `MvCameraControl.dll` through `raw-dylib`.
- At run time, MVS 4.7.0 or later must be installed and the directory of `MvCameraControl.dll` must be on `PATH`.

The vendor SDK, headers, import libraries and DLLs are not redistributed.

## License

MIT
