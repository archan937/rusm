//! A broker on `rusm_rs::pubsub::Topics`: registers `pubsub`, then routes JSON
//! commands — `{op:"sub",topic,pid}` / `{op:"pub",topic,data}` /
//! `{op:"count",topic,reply}`. Subscriber tracking, keyed fan-out, and dead-peer
//! pruning are entirely the primitive's job — this fixture is the thin domain shell,
//! demonstrating the infra/domain split the analysis prescribes.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log, "rusm:runtime/pg@0.1.0": rusm_rs::rusm::runtime::pg, "rusm:runtime/streams@0.1.0": rusm_rs::rusm::runtime::streams, "rusm:runtime/serve@0.1.0": rusm_rs::rusm::runtime::serve },
});

use rusm_rs::serde_json::{self, Value};

struct Component;

impl Guest for Component {
    fn run() {
        rusm_rs::register("pubsub");
        let mut topics = rusm_rs::pubsub::Topics::new();
        loop {
            let msg = rusm_rs::receive_bytes();
            // A monitored subscriber died → the primitive prunes it; nothing else to do.
            if topics.handle_down(&msg) {
                continue;
            }
            let Ok(cmd) = serde_json::from_slice::<Value>(&msg) else {
                continue;
            };
            let topic = cmd["topic"].as_str().unwrap_or_default();
            match cmd["op"].as_str() {
                Some("sub") => {
                    if let Some(pid) = cmd["pid"].as_u64() {
                        topics.subscribe(topic, rusm_rs::Pid(pid));
                    }
                }
                Some("pub") => {
                    topics.publish(topic, cmd["data"].as_str().unwrap_or_default().as_bytes());
                }
                Some("count") => {
                    if let Some(reply) = cmd["reply"].as_u64() {
                        let count = topics.subscriber_count(topic).to_string();
                        rusm_rs::send_bytes(rusm_rs::Pid(reply), count.as_bytes());
                    }
                }
                _ => {}
            }
        }
    }
}

export!(Component);
