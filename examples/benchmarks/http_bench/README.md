# `http_bench` — HTTP serving stress benchmark

Measures the **real** throughput + latency of serving a WASM component as an HTTP
handler, against a **bare-hyper baseline** (the same server loop returning a static
response, no Wasm) so the sandbox's cost is explicit — the number is *earned*, not
asserted.

```sh
# release, for real numbers:  [seconds] [keep-alive clients]
cargo run --release -p rusm-bench --example http_bench -- 5 64
```

Representative output (loopback, 64 clients):

```
lean WASM component (raw wasi:http):
  64537 req/sec   latency p50 980.4µs  p99 1717.4µs
  instantiate-only: 87928/sec = 11.4µs each

wstd WASM component (async reactor per request):
  51377 req/sec   latency p50 1222.5µs  p99 2134.8µs
  instantiate-only: 66470/sec = 15.0µs each

bare hyper (no Wasm, baseline):
  196965 req/sec   latency p50 321.3µs  p99 534.9µs

lean WASM vs bare hyper: 3.1x fewer req/s, +659.0µs p50  (the true sandbox cost)
```

## Reading it

- Each WASM server runs **one fresh, sandboxed component instance per request** —
  total isolation; a trap fails just that request.
- **The guest you write is the lever.** A lean raw-`wasi:http` component
  (`http_lean`) does ~64.5k req/s; the ergonomic `wstd` one (`http_hello`, with an
  async reactor per request) does ~51k — a ~26% difference with *zero* host change.
- **Instantiation is cheap (~11µs).** So per-request isolation is nearly free, and a
  warm-instance pool isn't worth trading isolation for. The residual ~3× vs bare
  hyper is the `wasi:http` component-model marshaling, not instantiation.
- Each client holds one keep-alive connection; the client reads full responses
  (Content-Length *or* chunked) so the connection stays in sync. `TCP_NODELAY` both
  ends.

Numbers scale with free CPU and client count. SSE and WS benchmarks (`sse-fanout`,
`ws-echo`) follow as those serving paths land.
