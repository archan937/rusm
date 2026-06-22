# rusm-cluster

> Secure distributed clustering for RUSM — connect nodes over QUIC + mutual TLS, with cross-node messaging, a gossiped global registry, remote spawn, and live attach.

`rusm-cluster` sits over the Wasm-free [`rusm-otp`](https://crates.io/crates/rusm-otp) core
(it never touches Wasmtime). A `ClusterNode` wraps a `Runtime` with a [quinn](https://crates.io/crates/quinn)
QUIC endpoint and rustls/ring TLS, turning a single-node actor runtime into a cluster.

## What it gives you

- **Mutual TLS** — a `ClusterCa` issues per-node certificates (or a shared self-signed
  `Identity`); foreign-CA peers are rejected.
- **Cross-node messaging** — peers exchange names on a per-peer control stream and route
  each message on its own uni-stream.
- **A gossiped global registry** — `register_global` / `whereis_global` / `send_global`.
- **Remote spawn** — named `Spawnable` factories, invoked over one request/reply control-plane RPC.
- **Live attach** — enumerate a peer's processes (`remote_pids`) to drive an observer/REPL.

~550k cross-node messages/sec, ~39µs p50 round-trip on loopback (the `cluster_fanout` benchmark).

Part of [RUSM](https://github.com/archan937/rusm). See the
[repo README](https://github.com/archan937/rusm#readme) and the
[architecture docs](https://github.com/archan937/rusm/blob/main/docs/about/architecture.md).
