//! A per-connection WebSocket handler that reports its connection context (path + query,
//! read via [`Connection::info`](rusm_rs::ws::Connection::info)) to the registered
//! `collector` on open — proving the `connection` op reaches a WS handler across the
//! upgrade (the WS twin of rs-sse-conn).
use rusm_rs::ws::{self, Connection, Handler};

struct Conn;

impl Handler for Conn {
    fn open(&mut self, conn: &Connection) {
        let i = conn.info();
        let report = format!("ctx {} q={}", i.path(), i.query());
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, report.as_bytes());
        }
    }

    // Echo nothing; this fixture only probes the connection context on open.
    fn message(&mut self, _conn: &Connection, _data: Vec<u8>) {}
}

#[rusm_rs::main]
fn run() {
    ws::serve(Conn);
}
