//! Embed a RUSM benchmark node in your own program and serve the live protocol.
//! Once running, point the dashboard or `rusm attach ws://127.0.0.1:4000` at it.
//!
//! Run: `cargo run -p rusm-bench --example embedded_node`

use rusm_bench::{serve, Node, RunnerConfig};

#[tokio::main]
async fn main() {
    let node = Node::new(RunnerConfig::default());
    let addr = "127.0.0.1:4000";
    println!("RUSM node on ws://{addr} — attach the dashboard or `rusm attach`");
    serve(addr, node).await.expect("serve");
}
