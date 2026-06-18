# Embedding RUSM

Use RUSM as a **Rust library** — from your own program, not the `rusm` CLI. Where the
[app examples](../) (`typescript`/`rust`/`go`) are guests you write and `rusm serve` hosts,
these drive the host crates directly: host WASM components as supervised processes, embed a
node, run a TypeScript guest, or build a cluster — all the raw API the CLI is built on.

Run any with:

```sh
cargo run -p rusm-bench --example <name>
```

| Example | Shows |
| --- | --- |
| [`host_components`](./host_components/) | **Host real WASM components** as isolated, introspectable, capability-sandboxed processes — `compile`→`prepare`→`spawn_component`, `Process.info`/`list`, and per-process memory caps (over-cap → trap → `Crashed`). The heart of the runtime. |
| [`host_ts_component`](./host_ts_component/) | A **TypeScript guest** as a first-class, sandboxed, message-passing process on the shared rquickjs js-runner — `WasmRuntime::spawn_js`, no per-component Wasm build. |
| [`embedded_node`](./embedded_node/) | **Embed a node** (`Node::new` + serve the control/observer WebSocket) inside your own program — the dashboard and `rusm attach` are just clients of that channel. |
| [`cluster`](./cluster/) | A **two-node cluster** over QUIC + mutual TLS: cross-node messaging, a location-hiding global registry, and live attach — `ClusterNode::bind` wrapping a normal `Runtime`. |

Each example's own README walks through the API it uses and the output to expect.
