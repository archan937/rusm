# RUSM examples

Three kinds of example, by what you're here for: **build an app** (start here),
**embed the runtime** as a library, or **see the numbers**.

## Apps — start here

### Smallest — a [URL shortener](./url-shortener/)

One handler over durable `kv` — `POST` a URL, get a short code; visit the code, get
redirected. The minimal complete app, in **TypeScript, Rust, and Go**, and the runnable
companion to the docs guide *A URL shortener*. Each variant uses published dependency specs,
so you can copy any one directory out of the repo and it still builds.

```sh
cd examples/url-shortener/<lang>     # typescript | rust | go
rusm build && rusm serve
curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'   # → /1
```

### Full — a collaborative [todo board](./typescript/)

The same app in each guest language — HTTP CRUD + a live SSE feed + WebSocket chat + a
service driven by a worker, each an isolated, supervised WASM process, unified by
process-group tags (no broker). Build and serve, then open `http://localhost:8080`:

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

### Native functions — a [weather custom bridge](./custom-bridge/)

Give every guest a typed function backed by host Rust. A `weather` bridge
(`bridges/weather/{bridge.wit,host.rs}`) is called from Rust, Go, **and** TypeScript guests as
an ordinary import — RUSM's answer to a capability provider, compiled-in and typed. Scaffold
your own with `rusm new <name> --bridges`.

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
