# Distributed nodes

A single RUSM node is a host process running many lightweight processes. A **cluster**
is several nodes — typically on different machines — connected so those processes can
spawn and message across the boundary as if they were local. That's distributed Erlang's
model: a process doesn't know (or care) which node a peer lives on. It ships in the
Wasm-free [`rusm-cluster`](https://github.com/archan937/rusm) crate, layered over
`rusm-otp` — so distribution never drags WebAssembly into the core.

## Connecting

Nodes connect over **QUIC** with **TLS 1.3** (think `Node.connect/1`, but secure by
default). Each node has a name; on connect, both ends exchange names over a
dedicated control stream and remember the peer. Links are **mutually
authenticated** — both ends present a certificate and verify the other against a
shared trust anchor, so a peer without a trusted certificate is rejected at the
handshake. Use a `ClusterCa` to issue each node its own CA-signed certificate
(per-node keys, independently revocable), or a single shared `Identity::generate()`
for a small/trusted cluster.

## Location transparency

Once connected:

- **Cross-node `send`** — `node.send("london", "greeter", bytes)` routes to the
  process registered as `greeter` on `london`; the sender doesn't open the socket
  itself.
- **Global registry** — `register_global(name, pid)` publishes a name cluster-wide
  by gossiping it to every peer; `send_global(name, bytes)` resolves the owning
  node and routes there, so a service is reachable by name from anywhere.
- **Remote spawn** — a node registers spawnable factories by name; a peer calls
  `spawn_remote(node, factory, args)` and gets back the pid spawned on that node.
  (A closure can't cross the wire, so a node only spawns work it's been taught —
  explicit and capability-friendly.)

## Wire shape

Each link multiplexes a single long-lived **control stream** (name exchange,
registry gossip, the request/reply RPC behind remote spawn and live attach) and one
**uni-stream per message** (so messages never head-of-line-block each other). On
loopback the transport does ~550k cross-node messages/sec at ~39µs p50 round-trip —
see the [`cluster_fanout`](https://github.com/archan937/rusm) benchmark.

## Testing it

Tests boot several nodes in one process and connect them on loopback, so cross-node
send, the global registry, remote spawn, and live-attach listing are all TDD-able
with no external network.

## Hooking in

You can also attach to any running node to inspect it live — see
[live attach](/deep-dive/live-attach).
