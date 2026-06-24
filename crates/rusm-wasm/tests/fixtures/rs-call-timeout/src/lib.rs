//! Calls a "black hole" service with a 50ms timeout and reports "timeout", "err:…",
//! or "got:…" to a collector. Exercises `rusm_rs::wire::call_timeout`.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: {
        "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor,
        "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv,
        "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log,
        "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg,
        "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams,
        "rusm:runtime/serve@0.1.0": rusm_rs::rusm::runtime::serve,
    },
});

struct Component;
export!(Component);

impl Guest for Component {
    fn run() {
        let hole = rusm_rs::Pid(
            String::from_utf8(rusm_rs::receive_bytes())
                .unwrap()
                .parse()
                .unwrap(),
        );
        let collector = rusm_rs::Pid(
            String::from_utf8(rusm_rs::receive_bytes())
                .unwrap()
                .parse()
                .unwrap(),
        );
        match rusm_rs::wire::call_timeout::<&str, String>(hole, "echo", &"hi", 50) {
            Err(e) if e == "timeout" => rusm_rs::send_bytes(collector, b"timeout"),
            Err(e) => rusm_rs::send_bytes(collector, format!("err:{e}").as_bytes()),
            Ok(v) => rusm_rs::send_bytes(collector, format!("got:{v}").as_bytes()),
        }
    }
}
