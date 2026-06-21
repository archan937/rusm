//! A guest that calls a **custom application bridge** — a typed host function the app
//! registered via `WasmRuntime::with_bridges`, not part of `rusm:runtime`. It receives a
//! reply-to pid, calls `demo:bridge/greet.greet("World")` (an ordinary WIT import, no
//! dispatcher), and sends the host's typed reply back. The standard rusm interfaces reuse
//! `rusm-rs`'s bindings; only `greet` is generated fresh here.

wit_bindgen::generate!({
    world: "greeter",
    path: "wit",
    with: {
        "rusm:runtime/types@0.1.0": rusm_rs::rusm::runtime::types,
        "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor,
        "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv,
        "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log,
        "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg,
        "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams,
        "rusm:runtime/serve@0.1.0": rusm_rs::rusm::runtime::serve,
        // The custom bridge: generate its binding fresh (it isn't part of rusm-rs).
        "demo:bridge/greet@0.1.0": generate,
    },
});

use demo::bridge::greet::greet;

struct Component;

impl Guest for Component {
    fn run() {
        let reply_to =
            rusm_rs::Pid(String::from_utf8(rusm_rs::receive_bytes()).unwrap().parse().unwrap());
        let answer = greet("World");
        rusm_rs::send_bytes(reply_to, answer.as_bytes());
    }
}

export!(Component);
