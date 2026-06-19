//! A per-connection SSE handler that emits a **rich** event (data + id + event name) via
//! [`Stream::emit`](rusm_rs::sse::Stream::emit) on open, then closes — proving the additive
//! `sse-send` op frames `id:`/`event:`/`data:`.
use rusm_rs::sse::{self, Event, Handler, Stream};

struct Greeter;

impl Handler for Greeter {
    fn open(&mut self, stream: &Stream) {
        stream.emit(&Event {
            data: b"hello",
            id: Some("42"),
            event: Some("greeting"),
            ..Default::default()
        });
        stream.close();
    }

    fn message(&mut self, _stream: &Stream, _event: Vec<u8>) {}
}

#[rusm_rs::main]
fn run() {
    sse::serve(Greeter);
}
