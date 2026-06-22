# Phase 9 — distributed clusters + live attach

**Goal:** make RUSM processes message each other **across nodes** — distributed
Erlang, for WebAssembly. Several independent nodes connect into a secure cluster;
a process reaches a service by name without knowing which node it lives on; a node
can spawn work on a peer and see what a peer is running. All over an encrypted,
authenticated transport.

## Why this matters

A single machine has limits; horizontal scale needs cheap, secure node-to-node
messaging. RUSM already had the actor model on one node (Phases 1–8). Phase 9 adds
the wire between nodes — and keeps the programming model identical: the same
`send`, the same registry, now spanning machines.

## What we built (TDD throughout)

The new Wasm-free **`rusm-cluster`** crate, layered over `rusm-otp` (no Wasmtime —
the hard boundary holds; distribution is an actor-core concern, not a Wasm one).

1. **QUIC + TLS transport.** A `ClusterNode` wraps a `Runtime` with a QUIC endpoint
   (quinn, rustls + ring). A cluster shares one self-signed `Identity` — a
   **pre-shared cluster certificate**: a node only completes a handshake with a peer
   presenting the same cert, and the client pins it as its sole trust root. A wrong
   certificate is rejected at the handshake. (Per-node certs under a cluster CA
   arrived later, in [Phase 10](./phase-10-scale-hardening.md) — mutual TLS, foreign-CA
   peers rejected; the transport seam didn't change.)
2. **Per-peer streams.** The handshake's **bidirectional stream stays open as a
   control channel** (node-name exchange, then registry gossip and a control-plane
   RPC); every **message rides its own uni-stream**, so cross-node messages never
   head-of-line-block each other — the reason to reach for QUIC over TCP.
3. **Cross-node messaging.** `node.send("london", "greeter", bytes)` routes to the
   process registered as `greeter` on `london`; a `RemoteNode` handle from
   `connect` does the same without naming the node.
4. **Global registry.** `register_global(name, pid)` registers locally and gossips
   ownership to every peer; a freshly-connected peer is **bootstrapped** with the
   names we already own, and late registrations **broadcast**. `send_global(name,
   bytes)` resolves the owning node and routes there — location transparency.
   `whereis_global` returns the owner. When a peer's control channel closes, its
   connection and the global names it owned are pruned.
5. **Remote spawn.** A node `register_spawnable(name, factory)`s work it can build;
   a peer calls `spawn_remote(node, factory, args)` and gets back the pid spawned
   *there*. The cluster can't ship a closure across the wire, so a node spawns only
   what it has been taught — explicit, and friendly to capability control.
6. **Live attach.** `remote_pids(node)` lists the processes alive on a peer — the
   cluster primitive behind attaching to a running node. Both remote spawn and live
   attach ride one **request/reply control-plane RPC** (correlated by id, awaited on
   a `oneshot`), handled off the gossip loop so a slow op never stalls a peer's
   registry sync.

## Performance

The [`cluster_fanout`](https://github.com/archan937/rusm/tree/main/examples/benchmarks/cluster_fanout) benchmark measures the
transport on loopback, in two honest phases (latency separately from throughput —
under saturation, latency is queue-dominated, so one number would mislead):

- **unloaded cross-node round-trip latency**: ~39µs p50 / ~112µs p99;
- **saturation throughput**: ~280k round-trips/sec ≈ **~550k cross-node
  messages/sec**.

## Concepts introduced

- [Distributed nodes](/deep-dive/distributed-nodes) — connecting, location
  transparency, the wire shape.
- [Live attach](/deep-dive/live-attach) — hooking into a running node.
- [Distributed model](/deep-dive/the-distributed-model) — the full reference.

## Play with it

```sh
# The smallest two-node cluster (cross-node send, global registry, live attach):
cargo run -p rusm-bench --example cluster

# Benchmark the cross-node transport (release, for real numbers):
cargo run --release -p rusm-bench --example cluster_fanout -- 5 4
```

## Verification

`cargo test -p rusm-cluster` green — cross-node delivery (by handle and by node
name), a single link carrying messages both ways, the global registry (gossip on
connect *and* on late registration, local fast-path), remote spawn (the factory ran
on the remote, its pid alive there) with an unknown-factory error, live-attach
listing of a peer's live pids, and wrong-certificate rejection — plus frame
parse/round-trip. `cargo fmt` + clippy clean. The Wasm-free invariant holds:
`rusm-cluster` depends on `rusm-otp`, never Wasmtime.

The dashboard's `distributed-fanout` scenario was **graduated to this real engine**
too (a hub + worker nodes, a sender pool keeping one round-trip in flight so latency
stays representative): live, it does ~71k round-trips/sec at ~105µs p50 on the
Balanced profile. With that, **every one of the nineteen dashboard scenarios now runs
on real data — none remain synthetic.** (`Runner::start_synthetic` keeps a
runtime-free deterministic preview mode for UI development.) The six serving scenarios
(HTTP/WS/SSE and their `*-ts` twins) are co-resident live demos driving a real
in-process WASM server; the fair, credible serving headline numbers are still measured
out-of-process by `rusm-loadtest` against a live `rusm serve` port.

## Next

[Phase 10](./phase-10-scale-hardening.md): **scale & hardening** — an on-demand
instance tier above the pooled ceiling, opt-in bounded mailboxes, and supervisor
restart-intensity.
