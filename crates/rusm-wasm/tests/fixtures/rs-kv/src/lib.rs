//! A guest using the ergonomic `rusm-rs` kv module. Same protocol as the raw-ABI
//! `actor-kv` fixture — receives `[reply-to pid: 8 LE]`, runs a CRUD sequence on the
//! `specs` bucket via `rusm_rs::kv`, and replies `[own pid: 8 LE][flags: 1 byte]`
//! (bit per correct step) — so the host asserts `0b111111` when storage is granted.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log, "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg, "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams },
});

struct Component;

impl Guest for Component {
    fn run() {
        let msg = rusm_rs::receive_bytes();
        let reply_to = rusm_rs::Pid(u64::from_le_bytes(msg[0..8].try_into().unwrap()));

        let specs = rusm_rs::kv::bucket("specs");
        let mut flags = 0u8;
        if specs.set("k", b"v1").is_ok() {
            flags |= 1 << 0;
            if specs.get("k") == Ok(Some(b"v1".to_vec())) {
                flags |= 1 << 1;
            }
            if specs.exists("k") == Ok(true) {
                flags |= 1 << 2;
            }
            if specs.list() == Ok(vec!["k".to_string()]) {
                flags |= 1 << 3;
            }
            if specs.delete("k") == Ok(true) {
                flags |= 1 << 4;
            }
            if specs.get("k") == Ok(None) {
                flags |= 1 << 5;
            }
        }

        let mut out = rusm_rs::me().0.to_le_bytes().to_vec();
        out.push(flags);
        rusm_rs::send_bytes(reply_to, &out);
    }
}

export!(Component);
