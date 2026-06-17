# rusm-wasm

> The Wasmtime backend for RUSM — runs each WebAssembly instance as a supervised, sandboxed [`rusm-otp`](https://crates.io/crates/rusm-otp) process.

`rusm-wasm` is the **only** crate in [RUSM](https://github.com/archan937/rusm) that touches
Wasmtime. It embeds Wasmtime as the per-process sandbox: one Wasm instance = one
lightweight process, so a guest crash stays contained and the OTP core stays Wasm-free.

## What it provides

- **Instance-per-process hosting** behind three bridges — **wasip1** (core modules + a raw
  `rusm::*` ABI), **wasip2** (the component model + the `rusm:runtime` WIT actor world: the
  Erlang `Process` API callable from any language), and **wasip3** (`@0.3.0` async WASI).
- **Default-deny capability profiles** (Sandboxed / NetworkClient / Trusted) gating
  fs / net / env / memory / spawn / storage, plus a `StoreLimiter` memory cap and epoch
  preemption.
- **Serving** — run a component as a high-throughput **HTTP / WS / SSE** server
  (instance-per-request / process-per-connection; hyper + `tokio-tungstenite` + `wasi:http`),
  with a capability-gated streaming outbound **`fetch`**.
- **Embedded guest runtimes** — the wizer-pre-initialized rquickjs **js-runner** (TS/JS
  guests as first-class processes) and **js-http-runner** (TS `fetch` handlers), shipped in-crate.
  Rust and Go (TinyGo) guests compile directly to `wasm32-wasip2` components — same actor world.
- **An optimized spawn path** — pooling allocator + copy-on-write + per-module `InstancePre`
  + precomputed export index — sustaining ~440k component spawns/sec.

```rust
use rusm_otp::Runtime;
use rusm_wasm::WasmRuntime;

let rt = Runtime::new();
let wr = WasmRuntime::new(rt.clone())?;
let handle = wr.spawn_component(&bytes)?;   // a .wasm component, now a supervised process
```

Part of [RUSM](https://github.com/archan937/rusm). See the
[repo README](https://github.com/archan937/rusm#readme) and the
[serving docs](https://github.com/archan937/rusm/blob/main/docs/serving-http-ws-sse.md).
