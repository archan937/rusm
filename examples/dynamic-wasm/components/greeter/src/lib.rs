//! A plugin: a normal compiled WASM actor component. It is **not** declared in `rusm.toml`
//! — `rusm build` compiles it to `wasm/greeter.wasm`, you publish that to the store
//! (`rusm kv set plugins/greeter wasm/greeter.wasm`), and the dispatcher spawns it by name
//! at runtime. It receives "<reply-to>\n<input>", computes, and answers.
use rusm_rs::{receive_bytes, send_bytes, Pid};

#[rusm_rs::main]
fn run() {
    let message = String::from_utf8(receive_bytes()).unwrap_or_default();
    let (reply_to, input) = message.split_once('\n').unwrap_or(("0", ""));
    let pid = Pid(reply_to.parse().unwrap_or(0));
    send_bytes(pid, format!("Hello, {input}!").as_bytes());
}
