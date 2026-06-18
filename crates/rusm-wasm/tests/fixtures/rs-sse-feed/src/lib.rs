//! A per-connection SSE handler (rusm-rs) that exercises the full lifecycle — `open`
//! (subscribe + report), `message` (emit each pushed event), and `close` (report) —
//! reporting "open"/"close" to a registered `collector` so a test can assert `close`
//! fires on disconnect. It subscribes to the "feed" process-group tag, so a publisher's
//! `whereis_tag("feed")` + `send` fans an event out to it (push-via-tags pub/sub).
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;

impl Feed {
    fn report(event: &[u8]) {
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, event);
        }
    }
}

impl Handler for Feed {
    fn open(&mut self, _stream: &Stream) {
        rusm_rs::register_tag("feed"); // subscribe to the feed topic
        Self::report(b"open");
    }
    fn message(&mut self, stream: &Stream, event: Vec<u8>) {
        if event == b"close" {
            stream.close(); // server-initiated self-stop
        } else {
            stream.data(&event); // emit the published event to the client
        }
    }
    fn close(&mut self, _stream: &Stream) {
        Self::report(b"close");
    }
}

#[rusm_rs::main]
fn run() {
    sse::serve(Feed);
}
