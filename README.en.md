# mvs-sdk

[中文](README.md)

Unofficial safe Rust wrapper for the Hikrobot MVS industrial camera SDK. The raw FFI lives in `mvs-sdk-sys`
and is re-exported as `mvs_sdk::sys`.

## Requirements

- Only `x86_64-pc-windows-msvc` is supported.
- Building does not need MVS installed.
- At run time, MVS 4.7.0 or later must be installed and the directory of `MvCameraControl.dll` must be on `PATH`
  (the MVS installer adds it by default). If the DLL cannot be found, the program exits with `0xC0000135`
  before `main` runs and prints nothing.

```toml
[dependencies]
mvs-sdk = "0.1"
```

## Example

```rust,no_run
use std::time::Duration;

use mvs_sdk::{AccessMode, Sdk, TransportLayer};

fn main() -> mvs_sdk::Result<()> {
    let sdk = Sdk::new()?;
    let devices = sdk.devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let mut camera = sdk.open(&devices[0], AccessMode::Exclusive, 0)?;
    camera.set_float(c"ExposureTime", 10_000.0)?;

    let grabbing = camera.start_grabbing()?;
    let buffer = grabbing.get_image_buffer(Some(Duration::from_secs(1)))?;
    let frame = buffer.frame();
    println!("{:?}, {} bytes", frame.info, frame.data.len());
    Ok(())
}
```

Complete examples are in [`examples/`](examples): device enumeration (`enumerate_devices`), pull grabbing
(`grab_pull`) and callback grabbing (`grab_callback`).

## Usage notes

- **Session**: a process has a single SDK session; `Sdk::new` and `clone` both return it. The SDK is finalized
  once the `Sdk` and every `Camera` are dropped, after which `Sdk::new` returns `Error::Finalized`.
- **Grabbing**: the guards returned by `start_grabbing` and `start_grabbing_with` borrow the camera mutably
  and stop grabbing when dropped. Nodes can still be read and written while grabbing.
- **Callbacks**: they run on an SDK thread, and a panic inside one aborts the process. Do not close the camera
  or stop grabbing inside a callback; hand that over to the thread that owns the camera through a channel.
- **Cleanup**: `Drop` ignores cleanup errors. Call `Camera::close`, `Grabbing::stop` or `CallbackGrabbing::stop`
  to check them.
- **Strings**: node names and similar arguments are `&CStr`, for example `c"ExposureTime"`.

See [`docs::architecture`](https://docs.rs/mvs-sdk/latest/mvs_sdk/docs/architecture/index.html) for details
(the API documentation is written in Chinese).

## Wrapped interfaces

Based on `MvCameraControl.h` of MVS 4.7.0, excluding the obsolete interfaces in `MvObsoleteInterfaces.h`:
31 of 144 interfaces are wrapped. The rest can be called through `mvs_sdk::sys`, with the handle from
`Camera::as_raw_handle` and the device record from `DeviceInfo::as_raw`.

| Part | Group | Interfaces | Wrapped |
| --- | --- | ---: | ---: |
| 1 | SDK initialization and version | 3 | 3 |
| 2 | Camera control and grabbing | 34 | 12 |
| 3 | Frame grabber configuration | 6 | 0 |
| 4 | Common camera/frame grabber properties | 28 | 12 |
| 5 | Camera and frame grabber upgrade | 2 | 0 |
| 6 | Exception callback and events | 6 | 4 |
| 7 | GigE only | 21 | 0 |
| 8 | CameraLink only | 6 | 0 |
| 9 | U3V only | 7 | 0 |
| 10 | GenTL | 4 | 0 |
| 11 | Image saving, conversion, processing and recording | 22 | 0 |
| 12 | Serial communication | 5 | 0 |

A failed SDK call returns `Error::Sdk { function, code }`, where `code` is an `ErrorCode`.

| Part | MVS SDK interface | Rust interface | Notes |
| --- | --- | --- | --- |
| 1 | `MV_CC_Initialize` | `Sdk::new() -> Result<Sdk>` | Initializes on the first call |
| 1 | `MV_CC_Finalize` | Called when the `Sdk` and every `Camera` are dropped |  |
| 1 | `MV_CC_GetSDKVersion` | `Sdk::version() -> u32` | No initialization needed |
| 2 | `MV_CC_EnumDevices` | `Sdk::devices(&self, TransportLayer) -> Result<Vec<DeviceInfo>>` |  |
| 2 | `MV_CC_IsDeviceAccessible` | `Sdk::is_accessible(&self, &DeviceInfo, AccessMode) -> bool` |  |
| 2 | `MV_CC_CreateHandle`, `MV_CC_OpenDevice` | `Sdk::open(&self, &DeviceInfo, AccessMode, u16) -> Result<Camera>` | The `u16` is the switchover key, only meaningful for native GigE devices |
| 2 | `MV_CC_IsDeviceConnected` | `Camera::is_connected(&self) -> bool` |  |
| 2 | `MV_CC_CloseDevice`, `MV_CC_DestroyHandle` | `Camera::close(self) -> Result<()>`, `Drop` |  |
| 2 | `MV_CC_RegisterImageCallBackEx2` | `Camera::start_grabbing_with(&mut self, F) -> Result<CallbackGrabbing<'_>>` | `F: Fn(Frame<'_>) + Send + Sync + 'static` |
| 2 | `MV_CC_StartGrabbing` | `Camera::start_grabbing(&mut self) -> Result<Grabbing<'_>>`, `start_grabbing_with` |  |
| 2 | `MV_CC_StopGrabbing` | `Grabbing::stop(self)`, `CallbackGrabbing::stop(self)`, `Drop` of the guards |  |
| 2 | `MV_CC_GetImageBuffer` | `Grabbing::get_image_buffer(&self, Option<Duration>) -> Result<FrameGuard<'_>>` | `None` waits forever |
| 2 | `MV_CC_FreeImageBuffer` | `Drop` of `FrameGuard` |  |
| 4 | `MV_CC_GetIntValueEx` | `Camera::get_int(&self, &CStr) -> Result<IntValue>` |  |
| 4 | `MV_CC_SetIntValueEx` | `Camera::set_int(&self, &CStr, i64) -> Result<()>` |  |
| 4 | `MV_CC_GetEnumValueEx` | `Camera::get_enum(&self, &CStr) -> Result<EnumValue>` |  |
| 4 | `MV_CC_SetEnumValue` | `Camera::set_enum(&self, &CStr, u32) -> Result<()>` |  |
| 4 | `MV_CC_SetEnumValueByString` | `Camera::set_enum_symbolic(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_GetFloatValue` | `Camera::get_float(&self, &CStr) -> Result<FloatValue>` |  |
| 4 | `MV_CC_SetFloatValue` | `Camera::set_float(&self, &CStr, f32) -> Result<()>` |  |
| 4 | `MV_CC_GetBoolValue` | `Camera::get_bool(&self, &CStr) -> Result<bool>` |  |
| 4 | `MV_CC_SetBoolValue` | `Camera::set_bool(&self, &CStr, bool) -> Result<()>` |  |
| 4 | `MV_CC_GetStringValue` | `Camera::get_string(&self, &CStr) -> Result<StringValue>` |  |
| 4 | `MV_CC_SetStringValue` | `Camera::set_string(&self, &CStr, &CStr) -> Result<()>` |  |
| 4 | `MV_CC_SetCommandValue` | `Camera::execute_command(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_RegisterExceptionCallBack` | `Camera::register_exception_callback(&mut self, F)`, `unregister_exception_callback(&mut self)` | `F: Fn(ExceptionKind) + Send + Sync + 'static` |
| 6 | `MV_CC_RegisterEventCallBackEx` | `Camera::register_event_callback(&mut self, &CStr, F)`, `unregister_event_callback(&mut self, &CStr)` | `F: Fn(EventInfo<'_>) + Send + Sync + 'static` |
| 6 | `MV_CC_EventNotificationOn` | `Camera::event_notification_on(&self, &CStr) -> Result<()>` |  |
| 6 | `MV_CC_EventNotificationOff` | `Camera::event_notification_off(&self, &CStr) -> Result<()>` |  |

## Structures

| MVS SDK structure | Rust type | Notes |
| --- | --- | --- |
| `MV_CC_DEVICE_INFO` | `DeviceInfo` | Strings are `&CStr`, GigE addresses are `Ipv4Addr`, the MAC is `[u8; 6]` |
| `MV_FRAME_OUT` | `FrameGuard<'_>`, `Frame<'_>` | `FrameGuard` returns the buffer when dropped; `Frame` borrows the pixels, copy `data` to keep them |
| `MV_FRAME_OUT_INFO_EX` | `FrameInfo` | Width and height prefer the extended fields, so sizes above 65535 work |
| `MV_EVENT_OUT_INFO` | `EventInfo<'_>` | Only valid during the callback |
| `MVCC_INTVALUE_EX` | `IntValue` |  |
| `MVCC_FLOATVALUE` | `FloatValue` |  |
| `MVCC_ENUMVALUE_EX` | `EnumValue` |  |
| `MVCC_STRINGVALUE` | `StringValue` |  |

## License

MIT. Use of the MVS SDK is subject to Hikrobot's license terms; this project does not redistribute any SDK files.
