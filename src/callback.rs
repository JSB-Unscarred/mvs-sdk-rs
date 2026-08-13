//! Public event metadata shared by every platform backend.

/// Borrowed view of an SDK event notification.
///
/// The value and its name are valid only for the event callback invocation.
/// Copy any fields needed by work that outlives the callback.
#[derive(Copy, Clone)]
pub struct EventInfo<'a> {
    name: &'a [u8],
    event_id: u16,
    stream_channel: u16,
    block_id: u64,
    timestamp: u64,
}

impl<'a> EventInfo<'a> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    pub(crate) fn new(
        name: &'a [u8],
        event_id: u16,
        stream_channel: u16,
        block_id: u64,
        timestamp: u64,
    ) -> Self {
        Self {
            name,
            event_id,
            stream_channel,
            block_id,
            timestamp,
        }
    }

    /// Return the event name, replacing invalid UTF-8 sequences if necessary.
    pub fn name(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.name)
    }

    /// Return the vendor event identifier.
    pub fn event_id(&self) -> u16 {
        self.event_id
    }

    /// Return the stream-channel identifier associated with the event.
    pub fn stream_channel(&self) -> u16 {
        self.stream_channel
    }

    /// Return the event block identifier.
    pub fn block_id(&self) -> u64 {
        self.block_id
    }

    /// Return the device event timestamp in vendor-defined units.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
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
