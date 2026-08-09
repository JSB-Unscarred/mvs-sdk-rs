use std::error::Error;
use std::io;

use mvs_sdk_rs::{Camera, DeviceInfo, DeviceList};

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

/// 从枚举结果中查找专用测试相机。
pub(crate) fn test_device<'list, 'sdk>(
    devices: &'list DeviceList<'sdk>,
) -> Result<&'list DeviceInfo<'sdk>, Box<dyn Error>> {
    let serial = test_camera_serial()?;
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

/// 要求相机处于 free-run，防止等待外部 trigger 导致测试超时。
pub(crate) fn require_trigger_off(camera: &Camera<'_>) -> Result<(), Box<dyn Error>> {
    if camera.get_enum("TriggerMode")?.current == 0 {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "the dedicated test camera must start with TriggerMode=Off",
    )
    .into())
}
