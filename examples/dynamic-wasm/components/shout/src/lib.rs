//! A second plugin, to show one sandbox runner serving many runtime-chosen compiled
//! components: same `plugin-runner` template, different `.wasm`. Publish it with
//! `rusm kv set plugins/shout wasm/shout.wasm`. It upper-cases the input.
use rusm_rs::{receive_bytes, send_bytes, Pid};

#[rusm_rs::main]
fn run() {
    let message = String::from_utf8(receive_bytes()).unwrap_or_default();
    let (reply_to, input) = message.split_once('\n').unwrap_or(("0", ""));
    let pid = Pid(reply_to.parse().unwrap_or(0));
    send_bytes(pid, input.to_uppercase().as_bytes());
}
