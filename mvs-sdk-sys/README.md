# mvs-sdk-sys

Raw, unsafe FFI bindings for the Hikvision MVS machine-vision camera SDK.
The bindings are generated with `bindgen` and are primarily an implementation
dependency of the safe `mvs-sdk-rs` crate. Application code should normally
depend on `mvs-sdk-rs` instead.

The native MVS SDK is currently supported on Windows x86_64. The generated
bindings are committed, so ordinary builds do not require libclang.

From the root of a repository checkout, maintainers can regenerate the Windows
x64 bindings with the explicit maintenance script:

```powershell
cargo install bindgen-cli --locked
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

`Bypass` applies only to that PowerShell child process and does not change the
system or user execution policy. The script requires Windows x64,
LLVM/libclang, and the MVS SDK. It reads the SDK development directory from
`MVCAM_COMMON_RUNENV` by default; pass `-SdkRoot <path>` to override it. The
directory must contain `Includes/MvCameraControl.h`.
