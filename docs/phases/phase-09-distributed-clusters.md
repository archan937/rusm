# Phase 9 — Distributed clusters + live attach

Same `send`. Same registry. Now spanning machines — Phase 9 connects RUSM nodes into a secure cluster where a process reaches any service by name, regardless of which node it lives on.

## Why this phase

A single machine has limits. Horizontal scale needs cheap, secure node-to-node messaging — and it needs the programming model to stay the same. The whole point of location transparency is that you don't rewrite your application logic to go distributed. You register a service by name. You send to that name. The cluster routes it.

Phase 9 delivers exactly that: extend the registry across nodes, route messages transparently, and secure the whole thing with QUIC + mutual TLS.

## What shipped

The new Wasm-free **`rusm-cluster`** crate, layered over `rusm-otp` (the hard boundary holds: no Wasmtime, distribution is an actor-core concern).

1. **QUIC + TLS transport** — `ClusterNode` wraps a `Runtime` with a QUIC endpoint (quinn + rustls + ring). The first version used a pre-shared cluster certificate (shared `Identity`); Phase 10 upgraded to per-node certs under a cluster CA with mutual authentication.
2. **Per-peer streams** — the handshake's bidirectional stream stays open as a control channel (node-name exchange, registry gossip, control-plane RPC). Every **message rides its own uni-stream** — cross-node messages never head-of-line-block each other. This is the reason to use QUIC over TCP.
3. **Cross-node messaging** — `node.send("london", "greeter", bytes)` routes to the process registered as `greeter` on `london`. A `RemoteNode` handle from `connect` does the same without naming the node explicitly.
4. **Global registry with gossip** — `register_global(name, pid)` registers locally and gossips ownership to every peer. A freshly-connected peer is bootstrapped with existing names; late registrations broadcast. `send_global(name, bytes)` resolves the owning node and routes there. `whereis_global` returns the owner. When a peer disconnects, its names are pruned.
5. **Remote spawn** — `spawn_remote(node, factory, args)` spawns work *on the peer* and returns the pid running there. A node can only spawn what it has been taught (`register_spawnable`) — explicit, friendly to capability control.
6. **Live attach** — `remote_pids(node)` lists the live processes on a peer. Both remote spawn and live attach ride one request/reply control-plane RPC, handled off the gossip loop so a slow op never stalls registry sync.

## Design highlights

- **QUIC instead of TCP for per-stream HoL-block freedom.** Each message in QUIC gets its own stream. A large or slow message doesn't delay unrelated messages sharing the same connection — which TCP multiplexing can't avoid. Under real load this matters: a registry gossip burst shouldn't delay latency-sensitive actor messages.
- **Gossip on connect, broadcast on register.** A new node joining the cluster immediately receives the full global registry state from its first peer — no catch-up protocol, no stale window. Late `register_global` calls broadcast to all current peers.
- **Control-plane RPC off the gossip loop.** Remote spawn and live attach are awaited on a `oneshot` channel, handled by a dedicated RPC path. A slow response to a `remote_pids` call cannot delay registry sync for other processes.
- **Wasm-free transport.** `rusm-cluster` depends on `rusm-otp`, never Wasmtime. Distribution is a runtime concern; sandboxing is a Wasm concern. The layers don't mix.

## What this unlocks

A multi-node RUSM application looks like a single-node one to the component developer. `register_global("payments", pid)` from any node makes `send_global("payments", msg)` work from any other node. Components don't know or care which node they're on.

The dashboard's `distributed-fanout` scenario now runs on this real engine — a hub + worker nodes, a sender pool keeping one round-trip in flight so latency stays representative. Every one of the 21 dashboard scenarios runs on real data as of this phase.

## Try it

```sh
# Two-node cluster — cross-node send, global registry, live attach:
cargo run -p rusm-bench --example cluster

# Benchmark the transport (release, for real numbers):
cargo run --release -p rusm-bench --example cluster_fanout -- 5 4
```

## Status

Phase complete. ~550k cross-node messages/sec; ~39µs p50 / ~112µs p99 unloaded round-trip latency. All 21 dashboard scenarios running on real data. Upgraded to per-node certs + mutual TLS in Phase 10.

---

*Next: [Phase 10](./phase-10-scale-hardening.md) — scale & hardening: on-demand instance overflow, bounded mailboxes, mutual TLS with per-node certs, and sliding-window restart intensity.*
