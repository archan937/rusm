# rusm-bench

> The benchmark + live-dashboard harness for RUSM — a repo-only dev tool (`publish = false`).

`rusm-bench` drives [RUSM](https://github.com/archan937/rusm)'s benchmark suite and the
live React dashboard. It hosts a benchmark **node** that runs scenarios over the real
`rusm-otp` / `rusm-wasm` engines and streams metrics out, and it backs the standalone
`cargo run -p rusm-bench --example <name>` benchmarks.

```sh
cargo run -p rusm-bench -- start                  # the dashboard/observer node (ws://127.0.0.1:4000)
cargo run -p rusm-bench -- run spawn-storm 5      # one scenario in the terminal, no node
cargo run --release -p rusm-bench --example component-storm   # a standalone benchmark
```

The dashboard scenarios are all **real** (none synthetic): spawn/message/supervision
throughput, the three serving shapes (HTTP/WS/SSE), clustering, durable `kv`, and pub/sub —
measured under genuine load. The standalone `examples/benchmarks/*` (registered here as
`[[example]]`s) report baseline-anchored numbers; the fair out-of-process serving headline
comes from [`rusm-loadtest`](../rusm-loadtest).

Part of [RUSM](https://github.com/archan937/rusm). See
[`docs/about/benchmark-dashboard-and-observer.md`](https://github.com/archan937/rusm/blob/main/docs/about/benchmark-dashboard-and-observer.md).
