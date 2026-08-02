//! Explicit smoke tests for a Windows x64 host with the MVS SDK and a dedicated
//! camera. These tests are excluded unless the `hardware-tests` feature is
//! enabled and remain ignored until requested by the operator.

#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

use std::error::Error;
use std::io;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mvs_sdk_rs::{Camera, MvsError, Sdk, TransportLayer};

const FRAME_TIMEOUT_MS: u32 = 3_000;

fn test_camera_serial() -> Result<String, io::Error> {
    std::env::var("MVS_TEST_CAMERA_SERIAL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "set MVS_TEST_CAMERA_SERIAL to the serial number of a dedicated test camera",
        )
    })
}

fn open_test_camera() -> Result<(Arc<Sdk>, Camera), Box<dyn Error>> {
    let serial = test_camera_serial()?;
    let sdk = Sdk::init()?;
    let devices = sdk.enumerate_devices(TransportLayer::GIGE | TransportLayer::USB)?;
    let device = devices
        .iter()
        .find(|device| device.serial() == serial)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("camera {serial:?} was not returned by the real SDK enumeration"),
            )
        })?;

    if !device.is_accessible(mvs_sdk_rs::AccessMode::Exclusive)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("camera {serial:?} is not available for exclusive access"),
        )
        .into());
    }

    let camera = device.open_exclusive()?;
    Ok((sdk, camera))
}

fn check_native(code: i32) -> Result<(), MvsError> {
    if code == mvs_sdk_sys::MV_OK as i32 {
        Ok(())
    } else {
        Err(MvsError::from(code))
    }
}

#[test]
#[ignore = "requires the MVS SDK, MVS_TEST_CAMERA_SERIAL, and TriggerMode=Off"]
fn real_enumeration_nodes_buffers_callbacks_and_shutdown() -> Result<(), Box<dyn Error>> {
    let (sdk, mut camera) = open_test_camera()?;

    let width = camera.get_int_range("Width")?;
    assert!((width.min..=width.max).contains(&width.current));
    assert!(width.inc > 0);

    let exposure = camera.get_float_range("ExposureTime")?;
    assert!(exposure.current.is_finite());
    assert!(exposure.min <= exposure.current && exposure.current <= exposure.max);

    let pixel_format = camera.get_enum_info("PixelFormat")?;
    assert!(pixel_format.supported.contains(&pixel_format.current));
    assert!(camera.is_connected());
    if camera.get_enum("TriggerMode")? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the dedicated test camera must start with TriggerMode=Off",
        )
        .into());
    }

    // This is a per-handle stream-buffer setting used only to establish the
    // two-buffer precondition. The safe acquisition API remains under test.
    let code = unsafe { mvs_sdk_sys::MV_CC_SetImageNodeNum(camera.as_raw_handle(), 2) };
    check_native(code)?;

    camera.start_grabbing()?;
    let first = camera.get_image_buffer(FRAME_TIMEOUT_MS)?;
    let second = camera.get_image_buffer(FRAME_TIMEOUT_MS)?;
    for guard in [&first, &second] {
        assert!(!guard.frame().data().is_empty());
        assert_eq!(
            guard.frame().data().len(),
            guard.info().frame_len() as usize
        );
    }
    first.release()?;
    drop(second);

    let third = camera.get_image_buffer(FRAME_TIMEOUT_MS)?;
    assert!(!third.frame().data().is_empty());
    third.release()?;
    camera.stop_grabbing()?;

    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    camera.register_image_callback(move |frame| {
        let _ = frame_tx.try_send(frame.to_owned());
    })?;
    camera.start_grabbing()?;
    let owned = frame_rx.recv_timeout(Duration::from_secs(3))?;
    assert!(!owned.data.is_empty());
    assert_eq!(owned.data.len(), owned.info().frame_len() as usize);
    camera.stop_grabbing()?;
    camera.unregister_image_callback()?;
    camera.close()?;
    sdk.shutdown()?;
    Ok(())
}
