//! Closure ↔ C-callback bridge (trampolines).
//!
//! The MVS SDK takes `extern "system" fn(..., user: *mut c_void)` pointers.
//! We want users to pass Rust closures. The plumbing:
//!
//! 1. Box the closure as `ImageCallback(Box<dyn FnMut(&Frame) + Send>)`.
//! 2. `Box` it again so we have a stable `*mut ImageCallback` address to
//!    hand to the SDK as `pUser`.
//! 3. Register a static `extern "system" fn` trampoline that casts the user
//!    pointer back to `&mut ImageCallback` and invokes the closure.
//! 4. [`Camera`] owns the outer `Box` and drops it after unregistering.
//!
//! [`Camera`]: crate::Camera

use std::os::raw::{c_uchar, c_uint, c_void};

use crate::frame::Frame;
use crate::sys;

// ---------------------------------------------------------------------------
// Callback wrappers (owning the closures)
// ---------------------------------------------------------------------------

pub(crate) struct ImageCallback(pub Box<dyn FnMut(&Frame<'_>) + Send + 'static>);

pub(crate) struct ExceptionCallback(pub Box<dyn FnMut(u32) + Send + 'static>);

pub(crate) struct EventCallback(pub Box<dyn FnMut(&EventInfo<'_>) + Send + 'static>);

// ---------------------------------------------------------------------------
// EventInfo (public)
// ---------------------------------------------------------------------------

/// Borrowed view of an SDK event notification. Valid only within the event
/// callback scope.
#[derive(Copy, Clone)]
pub struct EventInfo<'a>(&'a sys::MV_EVENT_OUT_INFO);

impl<'a> EventInfo<'a> {
    pub(crate) fn new(raw: &'a sys::MV_EVENT_OUT_INFO) -> Self {
        Self(raw)
    }

    pub fn name(&self) -> std::borrow::Cow<'_, str> {
        let bytes = &self.0.EventName;
        let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
        let slice =
            // SAFETY: c_char is signed or unsigned byte depending on platform;
            // on Windows it's i8. Reinterpret as u8 for UTF-8 decoding.
            unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u8, end) };
        String::from_utf8_lossy(slice)
    }

    pub fn event_id(&self) -> u16 {
        self.0.nEventID
    }
    pub fn stream_channel(&self) -> u16 {
        self.0.nStreamChannel
    }
    pub fn block_id(&self) -> u64 {
        ((self.0.nBlockIdHigh as u64) << 32) | self.0.nBlockIdLow as u64
    }
    pub fn timestamp(&self) -> u64 {
        ((self.0.nTimestampHigh as u64) << 32) | self.0.nTimestampLow as u64
    }
}

impl std::fmt::Debug for EventInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventInfo")
            .field("name", &self.name())
            .field("event_id", &self.event_id())
            .field("block_id", &self.block_id())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Trampolines
// ---------------------------------------------------------------------------

pub(crate) unsafe extern "C" fn image_trampoline(
    data: *mut c_uchar,
    info: *mut sys::MV_FRAME_OUT_INFO_EX,
    user: *mut c_void,
) {
    if user.is_null() || info.is_null() {
        return;
    }
    // SAFETY: `user` was set at registration to a `*mut ImageCallback` that is
    // pinned by `Camera` for as long as the callback remains registered. The
    // SDK guarantees `data`/`info` are valid for the duration of this call.
    unsafe {
        let cb = &mut *(user as *mut ImageCallback);
        let info_ref = &*info;
        let frame = Frame::from_raw_parts(data, info_ref);
        (cb.0)(&frame);
    }
}

pub(crate) unsafe extern "C" fn exception_trampoline(msg_type: c_uint, user: *mut c_void) {
    if user.is_null() {
        return;
    }
    // SAFETY: see image_trampoline.
    unsafe {
        let cb = &mut *(user as *mut ExceptionCallback);
        (cb.0)(msg_type as u32);
    }
}

pub(crate) unsafe extern "C" fn event_trampoline(
    info: *mut sys::MV_EVENT_OUT_INFO,
    user: *mut c_void,
) {
    if user.is_null() || info.is_null() {
        return;
    }
    // SAFETY: see image_trampoline.
    unsafe {
        let cb = &mut *(user as *mut EventCallback);
        let info_ref = &*info;
        let event = EventInfo::new(info_ref);
        (cb.0)(&event);
    }
}
