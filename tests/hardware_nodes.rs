//! Windows x64 + MVS SDK 真机节点访问测试。

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

mod hardware_support;

use std::error::Error;

// 验证 int、float、enum 三类常用节点可通过 safe API 读取有效范围。
#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and a dedicated camera"]
fn real_node_ranges_are_consistent() -> Result<(), Box<dyn Error>> {
    let (_sdk, camera) = hardware_support::open_test_camera()?;

    let width = camera.get_int_range("Width")?;
    assert!((width.min..=width.max).contains(&width.current));
    assert!(width.inc > 0);

    let exposure = camera.get_float_range("ExposureTime")?;
    assert!(exposure.current.is_finite());
    assert!(exposure.min <= exposure.current && exposure.current <= exposure.max);

    let pixel_format = camera.get_enum_info("PixelFormat")?;
    assert!(pixel_format.supported.contains(&pixel_format.current));

    camera.close()?;
    Ok(())
}
