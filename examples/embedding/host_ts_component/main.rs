//! **Running a TypeScript guest on RUSM's actor model** (rusm-ts).
//!
//! A TS component is plain TypeScript bundled by Bun and run on the shared
//! rquickjs js-runner — no per-component Wasm build. [`WasmRuntime::spawn_js`]
//! hands the bundle to a fresh, isolated, sandboxed process that gets the
//! `Process` actor API (and Web API polyfills). Here a TS worker receives a
//! reply-to pid, labels itself, and answers — proving a TS guest is a
//! first-class RUSM process, message-passing with the rest of the node.
//!
//! In a real app you'd write `components/worker/index.ts`, run `rusm build`
//! (Bun → `wasm/worker.js`), and declare it in `rusm.toml [components.worker]`. The
//! bundle below is exactly what `bun build --format=iife` emits for such a file.
//!
//! Run: `cargo run -p rusm-bench --example host_ts_component`

use rusm_otp::{Received, Runtime};
use rusm_wasm::WasmRuntime;

// What `bun build --format=cjs` produces from a worker `index.ts`:
//   export default async function () {
//     const replyTo = await Process.receiveText();
//     Process.setLabel("ts-worker");
//     Process.send(replyTo, `pong from ${Process.self()}`);
//   }
const BUNDLE: &str = r#"
  module.exports.default = async function () {
    const replyTo = await Process.receiveText();
    Process.setLabel("ts-worker");
    Process.send(replyTo, "pong from " + Process.self());
  };
"#;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let rt = Runtime::with_mailbox_depth();
    let wasm = WasmRuntime::new(rt.clone()).expect("wasm engine");

    // A collector process to receive the TS worker's reply.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let collector = rt.spawn(move |mut ctx| async move {
        if let Received::Message(bytes) = ctx.recv().await {
            let _ = tx.send(String::from_utf8(bytes).unwrap());
        }
    });

    // Spawn the TS bundle as a sandboxed process (default-deny capabilities).
    let worker = wasm.spawn_js(BUNDLE.as_bytes());
    println!("spawned a TS guest as process {:?}", worker.pid());
    if let Some(info) = rt.info(worker.pid()) {
        println!(
            "  label after it runs will be: ts-worker (currently {:?})",
            info.label
        );
    }

    // Tell the worker who to answer (decimal pid string), then await its reply.
    rt.send(worker.pid(), collector.pid().raw().to_string().into_bytes());
    println!("  TS guest replied: {:?}", rx.await.unwrap());

    println!("\nA TypeScript file ran as a sandboxed, addressable RUSM process.");
}
