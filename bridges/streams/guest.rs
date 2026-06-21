// Canonical source: bridges/streams/guest.rs — the stream bridge's Rust guest binding.
// Synced into rusm-rs (crates/rusm-rs/src/streams.rs) by `make sync-bridges`; edit this
// file, not the copy. The `bridge_guest_in_sync` test fails the build on drift.

//! Cross-process byte streams for a Rust guest — an ergonomic wrapper over the `stream`
//! bridge interface. The write end stays with the opener; the read end is delivered to the
//! target, which [`Stream::accept`]s it. `write`/`read`/`accept` suspend the fiber under
//! back-pressure / until data arrives (the host frees the Tokio worker meanwhile).

// The generated `streams` interface bindings.
use crate::rusm::runtime::streams as abi;
use crate::Pid;

/// A back-pressured byte stream to or from another process. Read suspends until a chunk
/// arrives; `None` is end-of-stream.
pub struct Stream {
    handle: u64,
}

impl Stream {
    /// Open a stream to a pid; `None` if the target is gone.
    pub fn open(to: Pid) -> Option<Stream> {
        abi::stream_open(to.0).map(|handle| Stream { handle })
    }

    /// Block until an incoming stream arrives, and take it for reading.
    pub fn accept() -> Stream {
        Stream {
            handle: abi::stream_accept(),
        }
    }

    /// Write one chunk; `false` once the reader is gone.
    pub fn write(&self, chunk: &[u8]) -> bool {
        abi::stream_write(self.handle, chunk)
    }

    /// Read the next chunk, or `None` at end-of-stream.
    pub fn read(&self) -> Option<Vec<u8>> {
        abi::stream_read(self.handle)
    }

    /// Close the write end (signals end-of-stream to the reader).
    pub fn close(self) {
        abi::stream_close(self.handle);
    }
}
