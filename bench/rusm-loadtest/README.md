# rusm-loadtest

> Out-of-process load driver for a live `rusm serve` node — the *fair* serving benchmark.
> A repo-only dev tool (`publish = false`).

`rusm-loadtest` measures [RUSM](https://github.com/archan937/rusm) serving throughput the
honest way: from a **separate process**, across a real socket, against a running `rusm serve`
port — so the load generator never shares the server's CPU and the number is the server's.

```sh
# terminal 1 — host an app (e.g. an examples/<lang> todo board):
rusm serve

# terminal 2 — drive load against it:
cargo run --release -p rusm-loadtest -- http http://127.0.0.1:8080
cargo run --release -p rusm-loadtest -- ws   ws://127.0.0.1:8082
cargo run --release -p rusm-loadtest -- sse  http://127.0.0.1:8081
cargo run --release -p rusm-loadtest -- conn ws://127.0.0.1:8082
```

Four modes: `http` (a [balter](https://crates.io/crates/balter) fixed-rate sweep —
req/s + tail latency + error rate), `ws` / `sse` (a connection-capacity harness holding
many connections and sustaining echo round-trips / draining events), and `conn` (a
connection-establishment storm — each a full sandboxed process-per-connection). These are
the source-of-truth serving numbers in the docs (the dashboard's co-resident tiles differ
by design — they share the node's CPU).

Part of [RUSM](https://github.com/archan937/rusm). See
[`docs/03-benchmark-dashboard.md`](https://github.com/archan937/rusm/blob/main/docs/03-benchmark-dashboard.md).
