//! A supervised child: tell the registered `collector` which pid we are, then
//! block until killed. Each (re)start announces a fresh pid, so a test can see
//! the supervisor restart us.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log, "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg, "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams },
});

struct Component;

impl Guest for Component {
    fn run() {
        let me = rusm_rs::me();
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, format!("started:{}", me.0).as_bytes());
        }
        loop {
            let _ = rusm_rs::receive_bytes(); // wait to be killed
        }
    }
}

export!(Component);
