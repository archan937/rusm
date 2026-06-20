//! A guest written with the ergonomic `rusm-rs` API, exercising its
//! receive-with-timeout helpers (Erlang's `receive … after`). On `run` it:
//!   1. learns who to answer (the first message, a decimal pid),
//!   2. calls `receive_bytes_timeout(30)` with nothing pending → expects `None`,
//!   3. signals `"armed"`, then `receive_timeout::<String>(5000)` → expects the
//!      host's JSON `"ping"` before the deadline,
//!   4. replies a single flags byte (bit 0 = timed out, bit 1 = got ping).
//! Together the two cases cover both helpers' timeout and delivery paths.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    // Reuse rusm-rs's actor import bindings instead of generating our own.
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv },
});

struct Component;

impl Guest for Component {
    fn run() {
        let reply_to = rusm_rs::Pid(
            String::from_utf8(rusm_rs::receive_bytes())
                .unwrap()
                .parse()
                .unwrap(),
        );

        // (1) Nothing pending: a short timeout must elapse → None.
        let timed_out = rusm_rs::receive_bytes_timeout(30).is_none();

        // Tell the host we are about to wait; it then sends JSON "ping".
        rusm_rs::send_bytes(reply_to, b"armed");

        // (2) A JSON message arriving before a long deadline must deserialize.
        let got_ping = matches!(
            rusm_rs::receive_timeout::<String>(5_000),
            Some(Ok(s)) if s == "ping"
        );

        let mut flags = 0u8;
        if timed_out {
            flags |= 1 << 0;
        }
        if got_ping {
            flags |= 1 << 1;
        }
        rusm_rs::send_bytes(reply_to, &[flags]);
    }
}

export!(Component);
