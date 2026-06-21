// Canonical source: bridges/streams/host.rs — the stream bridge's native host impl.
// Synced into rusm-wasm (crates/rusm-wasm/src/bridges/streams.rs) by `make sync-bridges`;
// edit this file, not the copy. The `bridge_host_in_sync` test fails the build on drift.

//! stream bridge — host side. Implements the `stream` WIT interface on [`WasiHost`] over
//! the Wasm-free `rusm-otp` byte-stream primitive: the write end lives here under a handle,
//! the read end is delivered to the target process (it `stream-accept`s, then `stream-read`s
//! chunks). `stream-write`/`-read`/`-accept` are **async** — they suspend the guest fiber
//! under back-pressure / until data or a stream arrives (never busy-spin), freeing the
//! Tokio worker (the "write blocking code, get async" property).

// The generated `streams` interface, aliased so it does not clash with `rusm_otp::stream`
// (the byte-stream constructor fn).
use crate::actor::rusm::runtime::streams as stream_iface;
use crate::bridges::WasiHost;
use rusm_otp::{stream, Pid, Received};

/// Wire the stream interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    stream_iface::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl stream_iface::Host for WasiHost {
    async fn stream_open(&mut self, to: u64) -> Option<u64> {
        let (writer, reader) = stream();
        if !self.rt.send_stream(Pid::from_raw(to), reader) {
            return None; // target gone
        }
        let id = self.next_stream;
        self.next_stream += 1;
        self.out_streams.insert(id, writer);
        Some(id)
    }

    async fn stream_write(&mut self, handle: u64, chunk: Vec<u8>) -> bool {
        // Clone the writer out so the await holds no borrow of the store.
        match self.out_streams.get(&handle).cloned() {
            Some(writer) => writer.write(chunk).await.is_ok(),
            None => false,
        }
    }

    async fn stream_close(&mut self, handle: u64) {
        self.out_streams.remove(&handle); // dropping the writer signals EOF
    }

    async fn stream_accept(&mut self) -> u64 {
        let ctx = self
            .ctx
            .as_mut()
            .expect("stream-accept runs inside a spawned process");
        // Like `receive`, deliver only streams here; skip plain messages/signals.
        let reader = loop {
            if let Received::Stream(handle) = ctx.recv().await {
                break handle;
            }
        };
        let id = self.next_stream;
        self.next_stream += 1;
        self.in_streams.insert(id, reader);
        id
    }

    async fn stream_read(&mut self, handle: u64) -> Option<Vec<u8>> {
        // Take the reader out (it isn't Clone — single consumer), await, re-insert
        // unless the stream has ended.
        let mut reader = self.in_streams.remove(&handle)?;
        match reader.read().await {
            Some(chunk) => {
                self.in_streams.insert(handle, reader);
                Some(chunk)
            }
            None => None, // end of stream
        }
    }
}
