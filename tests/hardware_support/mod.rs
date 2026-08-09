use std::error::Error;
use std::io;
use std::sync::Arc;

use mvs_sdk_rs::{AccessMode, Camera, DeviceInfo, MvsError, Sdk, TransportLayer};

pub(crate) const FRAME_TIMEOUT_MS: u32 = 3_000;

/// 读取专用测试相机序列号，避免测试误操作其他设备。
fn test_camera_serial() -> Result<String, io::Error> {
    std::env::var("MVS_TEST_CAMERA_SERIAL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "set MVS_TEST_CAMERA_SERIAL to the serial number of a dedicated test camera",
        )
    })
}

/// 枚举并返回指定相机的 Rust-owned 设备快照。
fn test_device(sdk: &Sdk) -> Result<DeviceInfo, Box<dyn Error>> {
    let serial = test_camera_serial()?;
    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    devices
        .iter()
        .find(|device| device.serial() == serial)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("camera {serial:?} was not returned by the real SDK enumeration"),
            )
            .into()
        })
}

/// 初始化 SDK、枚举并以 exclusive 模式打开指定相机。
pub(crate) fn open_test_camera() -> Result<(Arc<Sdk>, Camera), Box<dyn Error>> {
    let sdk = Sdk::init()?;
    let device = test_device(&sdk)?;
    if !device.is_accessible(AccessMode::Exclusive)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "camera {:?} is not available for exclusive access",
                device.serial()
            ),
        )
        .into());
    }

    let camera = device.open_exclusive()?;
    Ok((sdk, camera))
}

/// 要求相机处于 free-run，防止等待外部 trigger 导致测试超时。
pub(crate) fn require_trigger_off(camera: &Camera) -> Result<(), Box<dyn Error>> {
    if camera.get_enum("TriggerMode")? == 0 {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "the dedicated test camera must start with TriggerMode=Off",
    )
    .into())
}

/// 设置两个 stream buffer，使单次归还可在另一 buffer 被占用时验证。
pub(crate) fn set_two_stream_buffers(camera: &Camera) -> Result<(), MvsError> {
    // SAFETY: camera 持有 live handle，调用发生在 acquisition 停止期间，
    // 且该接口只修改当前 handle 的 stream-buffer 数量。
    let code = unsafe { mvs_sdk_sys::MV_CC_SetImageNodeNum(camera.as_raw_handle(), 2) };
    check_native(code)
}

/// 将测试所需的 raw SDK 返回码转换为 safe crate 错误。
fn check_native(code: i32) -> Result<(), MvsError> {
    if code == mvs_sdk_sys::MV_OK as i32 {
        Ok(())
    } else {
        Err(MvsError::from(code))
    }
}
