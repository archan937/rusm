# Embedding RUSM as a library

The app model is how most projects use RUSM. But the runtime is also a plain Rust
library you can drive directly from your own binary — useful when RUSM is one piece of a
larger app. Add the crates:

```sh
cargo add rusm-otp           # the Wasm-free actor core
cargo add rusm-wasm          # + the Wasmtime backend, to host components
```

## The OTP core, without any Wasm

RUSM's heart is a **Wasm-free** Erlang/OTP actor library, `rusm-otp` — real lightweight
processes, message passing, links, monitors, supervision, a registry, and timers, with **no
WebAssembly at all** (the dependency graph guarantees it stands alone):

```rust
use rusm_otp::{ExitReason, Received, Runtime};

#[tokio::main]
async fn main() {
    let rt = Runtime::new();

    // A worker: receive one message, then exit.
    let worker = rt.spawn(|mut ctx| async move {
        if let Received::Message(bytes) = ctx.recv().await {
            println!("worker got {} bytes", bytes.len());
        }
    });

    // Supervise it: monitor delivers a `Down` with the exit reason.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let watcher = rt
        .spawn(move |mut ctx| async move {
            if let Received::Down { reason, .. } = ctx.recv().await {
                let _ = tx.send(reason);
            }
        })
        .pid();
    rt.monitor(watcher, worker.pid());

    rt.send(worker.pid(), b"hello".to_vec()); // messages are bytes (Vec<u8>)
    assert_eq!(rx.await.unwrap(), ExitReason::Normal);
}
```

You also get `spawn_link` (crash propagation), `trap_exit`, a named **registry**
(`register`/`whereis`), **timers** (`send_after`/`cancel`), graceful `shutdown`, and **TCP**
(`listen`/`connect`, one process per connection) — all in `rusm-otp`, all without touching
Wasm. See [links & supervision](./concepts/links-and-supervision).

## Hosting a compiled component

Add the `rusm-wasm` backend and host a prebuilt component as a process. A `WasmRuntime` wraps
an `rusm-otp` `Runtime`; **construct it inside a Tokio runtime** (it starts the epoch ticker):

```rust
use rusm_otp::Runtime;
use rusm_wasm::{Capabilities, WasmRuntime};

#[tokio::main]
async fn main() {
    let rt = Runtime::new();
    let wasm = WasmRuntime::new(rt.clone()).unwrap();

    // compile once → prepare once (imports + entry export resolved) → spawn many.
    let bytes = std::fs::read("wasm/worker.wasm").unwrap();
    let prepared = wasm
        .prepare_component(&wasm.compile_component(&bytes).unwrap(), "run")
        .unwrap();

    // Default-deny Sandboxed profile…
    wasm.spawn_component(&prepared).join().await;

    // …or grant capabilities explicitly (here: an 8 MiB heap cap):
    let caps = Capabilities::nothing().max_memory(8 << 20);
    wasm.spawn_component_with(&prepared, caps).join().await;
}
```

A trap (or a denied capability the guest turns into a trap) exits the process `Crashed`, so
links and supervisors react exactly as for a native process. The runnable
[`host_components`](https://github.com/archan937/rusm/tree/main/examples/embedding/host_components)
example (`make example EX=host_components`) shows this end to end, including a memory-cap
denial.

> **Core modules.** A `wasm32-wasip1` core module works the same way with `compile` /
> `prepare(module, "run")` / `spawn` (see the wasip1 bridge).
