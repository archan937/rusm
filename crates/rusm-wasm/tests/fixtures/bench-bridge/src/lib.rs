//! The `custom-bridge` benchmark guest: it calls a **custom application bridge**
//! (`demo:bridge/greet`, a native host function wired via `WasmRuntime::with_bridges`) in a
//! tight **loop** — one call per request — so the dashboard measures sustained native-bridge
//! round-trips, not one-shot spawn cost. It receives its driver's pid once, then forever:
//! await a "go" token, call the bridge, reply with the host's typed answer.

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
        "demo:bridge/greet@0.1.0": generate,
    },
});

use demo::bridge::greet::greet;

struct Component;

impl Guest for Component {
    fn run() {
        let driver =
            rusm_rs::Pid(String::from_utf8(rusm_rs::receive_bytes()).unwrap().parse().unwrap());
        loop {
            let _ = rusm_rs::receive_bytes(); // a "go" token
            let answer = greet("World"); // the native host bridge call
            rusm_rs::send_bytes(driver, answer.as_bytes());
        }
    }
}

export!(Component);
