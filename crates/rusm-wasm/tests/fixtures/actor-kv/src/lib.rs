//! A guest fixture exercising the `kv-*` actor ABI (durable key-value storage).
//! On `run` it receives `[reply-to pid: 8 LE]`, runs a full CRUD sequence against
//! the `specs` bucket, and replies `[own pid: 8 LE][flags: 1 byte]` — one bit per
//! step that behaved correctly:
//!   bit0 set  ·  bit1 get="v1"  ·  bit2 exists  ·  bit3 list=["k"]  ·
//!   bit4 delete(true)  ·  bit5 get=none after delete.
//! When storage is denied (capability or no store), `kv-set` errs and *no* bit is
//! set — so the host asserts `0b111111` when granted and `0` when refused.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
});

use rusm::runtime::actor;
use rusm::runtime::kv;

struct Component;

impl Guest for Component {
    fn run() {
        let msg = actor::receive();
        let reply_to = u64::from_le_bytes(msg[0..8].try_into().unwrap());

        let mut flags = 0u8;
        if kv::set("specs", "k", b"v1").is_ok() {
            flags |= 1 << 0;
            if kv::get("specs", "k") == Ok(Some(b"v1".to_vec())) {
                flags |= 1 << 1;
            }
            if kv::exists("specs", "k") == Ok(true) {
                flags |= 1 << 2;
            }
            if kv::list("specs") == Ok(vec!["k".to_string()]) {
                flags |= 1 << 3;
            }
            if kv::delete("specs", "k") == Ok(true) {
                flags |= 1 << 4;
            }
            if kv::get("specs", "k") == Ok(None) {
                flags |= 1 << 5;
            }
        }

        let mut out = actor::own_pid().to_le_bytes().to_vec();
        out.push(flags);
        actor::send(reply_to, &out);
    }
}

export!(Component);
