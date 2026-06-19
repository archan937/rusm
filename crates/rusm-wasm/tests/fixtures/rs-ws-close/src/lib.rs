//! A WebSocket handler that closes the connection with a status code + reason via
//! [`Connection::close`](rusm_rs::ws::Connection::close) on the first inbound frame —
//! proving the additive `ws-close` op delivers a close frame with the code.
use rusm_rs::ws::{self, Connection, Handler};

struct Closer;

impl Handler for Closer {
    fn message(&mut self, conn: &Connection, _data: Vec<u8>) {
        conn.close(1000, "bye"); // normal closure
    }
}

#[rusm_rs::main]
fn run() {
    ws::serve(Closer);
}
