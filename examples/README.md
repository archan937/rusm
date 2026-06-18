# RUSM examples

Three kinds of example, by what you're here for: **build an app** (start here),
**embed the runtime** as a library, or **see the numbers**.

## Apps — start here

A complete **collaborative todo board**, one per guest language — the same app, idiomatic
to each. HTTP CRUD + a live SSE feed + WebSocket chat + a service driven by a worker, each
an isolated, supervised WASM process, unified by process-group tags (no broker). Build and
serve, then open `http://localhost:8080`:

```sh
cd examples/<lang>          # typescript | rust | go
rusm build && rusm serve
```

| App | Language | Toolchain |
| --- | --- | --- |
| [`typescript`](./typescript/) | TypeScript (Bun) | `rusm build` bundles each component |
| [`rust`](./rust/) | Rust | `cargo` → `wasm32-wasip2` |
| [`go`](./go/) | Go | TinyGo → `wasm32-wasip2` |

Each app's README has the full tour (the five components, the web page, the composition).
They're the on-ramp: write normal code in your language, get a supervised multi-protocol
server.

## [Embedding](./embedding/) — use RUSM as a Rust library

Drive the host crates directly, the way the CLI does: host WASM components as supervised
processes, run a TypeScript guest, embed a node, or build a cluster.
`host_components` · `host_ts_component` · `embedded_node` · `cluster` — see
[`embedding/README.md`](./embedding/).

## [Benchmarks](./benchmarks/) — performance, measured not asserted

Throughput, tail latency, and capacity against real baselines (bare hyper, a host echo).
`http_bench` · `ws_bench` · `sse_bench` · `connection_scale` · `cluster_fanout` — see
[`benchmarks/README.md`](./benchmarks/).

## End-to-end recipes

Start a node, then watch it from the dashboard and/or a REPL:

```sh
# 1. Start a node (or run the embedded_node example)
cargo run -p rusm-cli -- node start            # ws://127.0.0.1:4000

# 2a. The dashboard
cd bench/dashboard && bun install && bun run dev

# 2b. …or a live REPL (like `iex --remsh`); no URL needed for the local node
cargo run -p rusm-cli -- attach
#   run connection-storm
#   detail off
#   stop
#   quit

# Or run a scenario straight in the terminal, no node:
cargo run -p rusm-bench -- run connection-storm 5
```
