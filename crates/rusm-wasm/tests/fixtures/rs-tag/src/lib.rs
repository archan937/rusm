//! A guest fixture exercising **process-group tags** through the `rusm-rs` API. The first
//! message is the reply-to pid (decimal); the second is a command `"verb arg"`:
//!   - `tag NAME`   — `register_tag(NAME)`, reply `"tagged"`, then idle (stay in the group).
//!   - `kill NAME`  — reply `kill_tag(NAME)` as a decimal count (needs process-control).
//!   - `count NAME` — reply `whereis_tag(NAME).len()` as a decimal.

wit_bindgen::generate!({
    world: "process",
    path: "wit",
    // Reuse rusm-rs's actor import bindings instead of generating our own.
    with: { "rusm:runtime/actor@0.1.0": rusm_rs::rusm::runtime::actor, "rusm:runtime/kv@0.1.0": rusm_rs::rusm::runtime::kv, "rusm:runtime/log@0.1.0": rusm_rs::rusm::runtime::log },
});

struct Component;

/// The next message as a UTF-8 line.
fn line() -> String {
    String::from_utf8(rusm_rs::receive_bytes()).unwrap()
}

impl Guest for Component {
    fn run() {
        let reply_to = rusm_rs::Pid(line().parse().unwrap());
        let cmd = line();
        let (verb, arg) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        match verb {
            "tag" => {
                rusm_rs::register_tag(arg);
                rusm_rs::send_bytes(reply_to, b"tagged");
                loop {
                    let _ = rusm_rs::receive_bytes(); // stay alive as a group member
                }
            }
            "kill" => {
                let killed = rusm_rs::kill_tag(arg);
                rusm_rs::send_bytes(reply_to, killed.to_string().as_bytes());
            }
            "count" => {
                let n = rusm_rs::whereis_tag(arg).len();
                rusm_rs::send_bytes(reply_to, n.to_string().as_bytes());
            }
            _ => {}
        }
    }
}

export!(Component);
