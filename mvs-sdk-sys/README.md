# mvs-sdk-sys

Unofficial raw FFI bindings for the Hikrobot MVS industrial camera SDK (`MvCameraControl`),
baseline MVS 4.7.0. Most applications should use the safe [`mvs-sdk`](https://crates.io/crates/mvs-sdk)
crate, which re-exports this crate as `mvs_sdk::sys`.

## Requirements

- Target: `x86_64-pc-windows-msvc` only; other targets fail at compile time.
- Build: the bindings link `MvCameraControl.dll` through `raw-dylib`, so building needs neither the SDK
  nor its import library.
- Run: the directory containing `MvCameraControl.dll` must be on `PATH` (the MVS installer adds it).

## Bindings

The bindings are generated with bindgen 0.72.1 and committed, so ordinary builds do not need libclang.
Maintainers regenerate them from the root of a repository checkout after an SDK update:

```powershell
cargo install bindgen-cli --version 0.72.1 --locked
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\generate-bindings-windows-x64.ps1
```

`Bypass` applies only to that PowerShell child process. The script requires LLVM/libclang and the MVS SDK,
reads the SDK development directory from `MVCAM_COMMON_RUNENV` (override with `-SdkRoot <path>`), merges the
extern blocks and adds the `raw-dylib` link attribute. The pinned bindgen version matches the header of the
committed bindings and avoids unrelated output churn.

The vendor SDK, headers, import libraries and DLLs are not redistributed.

## License

MIT
