//! A WebSocket handler that replies with a **text** frame (via
//! [`Connection::send_text`](rusm_rs::ws::Connection::send_text)) — proving the additive
//! `ws-send-text` op delivers a text-opcode frame, where the default `send` is binary.
use rusm_rs::ws::{self, Connection, Handler};

struct Echo;

impl Handler for Echo {
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        // Echo the inbound payload back as a TEXT frame (not the default binary).
        conn.send_text(&String::from_utf8_lossy(&data));
    }
}

#[rusm_rs::main]
fn run() {
    ws::serve(Echo);
}
