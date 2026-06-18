//! A per-connection WebSocket handler (rusm-rs) that exercises the full lifecycle —
//! `open`, `message` (echo), and `close` — reporting "open"/"close" to a registered
//! `collector` process so a test can assert `close` fires when the socket drops.
use rusm_rs::ws::{self, Connection, Handler};

struct Lifecycle;

impl Lifecycle {
    fn report(event: &[u8]) {
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, event);
        }
    }
}

impl Handler for Lifecycle {
    fn open(&mut self, _conn: &Connection) {
        Self::report(b"open");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        conn.send(&data); // echo the frame back
    }
    fn close(&mut self, _conn: &Connection) {
        Self::report(b"close");
    }
}

#[rusm_rs::main]
fn run() {
    ws::serve(Lifecycle);
}
