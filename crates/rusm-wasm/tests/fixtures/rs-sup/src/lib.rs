//! A supervisor component: spawn + monitor the `flaky` child and restart it
//! one-for-one when it dies — with **restart intensity**: give up if more than 2
//! restarts happen within an hour. A single kill restarts the child; a rapid burst
//! of kills trips the limit and the supervisor itself exits.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log, "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg, "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams, "rusm:runtime/serve@0.1.0": rusm_rs::rusm::runtime::serve },
});

struct Component;

impl Guest for Component {
    fn run() {
        rusm_rs::Supervisor::new(rusm_rs::Strategy::OneForOne)
            .child("flaky")
            .max_restarts(2)
            .within(std::time::Duration::from_secs(3600))
            .run();
    }
}

export!(Component);
