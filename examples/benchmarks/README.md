# Benchmarks

Performance proofs for the RUSM runtime — **measured, not asserted**. Each holds load
against a real engine and reports throughput, tail latency, and capacity; the serving
ones run beside a **baseline** (bare hyper, or a host echo with no Wasm) so the sandbox's
cost is explicit and *earned*, not hand-waved.

Run any with `--release` for real numbers (debug builds are not representative):

```sh
cargo run --release -p rusm-bench --example <name> -- [args]
```

| Example | Measures | Headline (loopback, everyday load) |
| --- | --- | --- |
| [`http_bench`](./http_bench/) | HTTP serving (`wasi:http` per request) vs a bare-hyper baseline; lean vs wstd; instantiate-only rate | ~64.5k req/s lean; the true sandbox cost vs bare hyper |
| [`ws_bench`](./ws_bench/) | WebSocket echo: a sandboxed component **process per connection** vs a host echo (transport ceiling) | ~192k round-trips/s, 128 connections held |
| [`sse_bench`](./sse_bench/) | Many long-lived `text/event-stream` connections, one component instance each | streams held + total events/s; per-instance teardown |
| [`connection_scale`](./connection_scale/) | How many **held-open** connections coexist (the fd-bound concurrency ceiling) | tens of thousands of held connections, each its own process |
| [`cluster_fanout`](./cluster_fanout/) | Cross-node messaging over QUIC+TLS: unloaded round-trip latency + saturation throughput | ~552k cross-node msgs/s, ~39µs p50 |

Each example's own README explains how it measures and how to read the output. These are
the standalone, baseline-anchored benchmarks; the live dashboard
(`bench/dashboard`) and the out-of-process serving load test (`bench/rusm-loadtest`,
against a real `rusm serve` port) are the other two ways the same numbers are produced.
