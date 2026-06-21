//! A guest fixture proving **cross-process byte streaming** through the
//! `rusm:runtime` actor world. On `run` it reads `[role: 1 byte][peer pid: 8 LE]`:
//!   - role 0 (producer): open a stream to the peer, write 3 chunks of "hello!"
//!     (18 bytes), then close it.
//!   - role 1 (consumer): accept the stream, read chunks to end-of-stream summing
//!     their lengths, then send the total (u32 LE) to the peer.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
});

use rusm::runtime::actor;
use rusm::runtime::streams;

struct Component;

impl Guest for Component {
    fn run() {
        let msg = actor::receive();
        let role = msg[0];
        let peer = u64::from_le_bytes(msg[1..9].try_into().unwrap());

        if role == 0 {
            // Producer: stream 3 x "hello!" to the peer, then close.
            if let Some(id) = streams::stream_open(peer) {
                for _ in 0..3 {
                    streams::stream_write(id, b"hello!");
                }
                streams::stream_close(id);
            }
        } else if role == 1 {
            // Consumer: accept the stream, read to EOF, report the byte total.
            let id = streams::stream_accept();
            let mut total: u32 = 0;
            while let Some(chunk) = streams::stream_read(id) {
                total += chunk.len() as u32;
            }
            actor::send(peer, &total.to_le_bytes());
        } else {
            // Error paths: open to a dead pid, write/read bogus handles. Report a
            // flags byte: bit0 open=none, bit1 write=false, bit2 read=none.
            let mut flags = 0u32;
            if streams::stream_open(999_999).is_none() {
                flags |= 1;
            }
            if !streams::stream_write(123, b"x") {
                flags |= 2;
            }
            if streams::stream_read(456).is_none() {
                flags |= 4;
            }
            actor::send(peer, &flags.to_le_bytes());
        }
    }
}

export!(Component);
