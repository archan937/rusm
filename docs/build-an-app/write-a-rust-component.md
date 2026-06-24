# Write a Rust component

Write the source, let RUSM build it. A component lives under `components/<name>/`:

```
my-app/
├── rusm.toml
├── components/
│   └── worker/
│       ├── Cargo.toml      # crate-type = ["cdylib"], wit-bindgen
│       ├── wit/            # the rusm:runtime world (vendored from crates/rusm-wasm/wit)
│       └── src/lib.rs
└── wasm/                   # rusm build writes worker.wasm here
```

`src/lib.rs` binds the `rusm:runtime` actor world with `wit-bindgen` and exports
`run`:

```rust
wit_bindgen::generate!({ world: "process", path: "wit" });

use rusm::runtime::actor;

struct Component;

impl Guest for Component {
    fn run() {
        actor::set_label("worker");
        let msg = actor::receive();              // block for a message (bytes)
        actor::send(actor::own_pid(), &msg);     // echo to self, etc.
    }
}

export!(Component);
```

Build and run the whole app:

```sh
rusm build        # cargo build --target wasm32-wasip2 per components/* → ./wasm/
rusm run          # spawn them per rusm.toml
rusm dev          # build + run, then watch ./components and reload on edit
```

One toolchain, no jco, no cargo-component — `cargo build --target wasm32-wasip2`
componentizes directly. **`rusm dev`** keeps running: edit a component and save,
and it rebuilds + reloads it automatically (a dependency-free mtime watch).

## A service + typed client

The raw `wit-bindgen` shell above is the floor; for real components the **`rusm-rs`** crate
gives the ergonomic surface — `Pid` / `send` / `receive` (serde) / `spawn` / registry /
`Stream`. A **service** is a module of free functions under `#[rusm_rs::service]`: the macro
generates the receive → dispatch → reply loop **and** a typed `Client`, so a caller reaches it
with an ordinary method call that's really a cross-process message:

```rust
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n }   // streaming
    pub fn work(progress: rusm_rs::Callback<i64>) -> String {        // callback
        for pct in [25, 50, 100] { progress.call(pct); }
        "done".into()
    }
}

// caller (another component):
//   let calc = calc::Client::spawn("calc")?;   // spawn-from-guest, capability-gated
//   let sum  = calc.add(2, 3)?;                 // call: spawn + send + receive, hidden
```

Declare both in `rusm.toml` under `[components.<name>]` (the caller needs the `allow-spawn`
capability). It's the **same JSON wire** as `rusm-ts` and `rusm-go`, so a Rust client and a TS
or Go service interoperate. Errors are ordinary `Result`s; logging is the standard `log` crate,
routed to the node's log stream — the `#[rusm_rs::main]` / `#[handlers]` entry points install
the sink for you, and the host stamps the time, `component#pid`, and severity (no name/pid
wiring, no `allow-stdio`).

To serve a Rust component over HTTP/WS/SSE, see
[Serve HTTP](/build-an-app/serve-http). The runnable
[`rust`](https://github.com/archan937/rusm/tree/main/examples/todo-board/rust) todo-board example wires
the same model end to end.
