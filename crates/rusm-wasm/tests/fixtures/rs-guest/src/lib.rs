//! A guest written with the ergonomic `rusm-rs` API. The only binding boilerplate
//! is `generate!` (mapping the actor import to rusm-rs's, so it's imported once)
//! plus `export!`; everything else uses `rusm_rs::*`. On `run` it learns who to
//! answer (the first message, a decimal pid), labels itself, and replies.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    // Reuse rusm-rs's actor import bindings instead of generating our own.
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log, "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams },
});

struct Component;

impl Guest for Component {
    fn run() {
        let reply_to: u64 = String::from_utf8(rusm_rs::receive_bytes())
            .unwrap()
            .parse()
            .unwrap();
        rusm_rs::set_label("rs-guest");
        let msg = format!("hello from {}", rusm_rs::me());
        rusm_rs::send_bytes(rusm_rs::Pid(reply_to), msg.as_bytes());
    }
}

export!(Component);
