# `cluster` — a two-node RUSM cluster

The smallest example that shows RUSM processes messaging each other **across
nodes**, over the QUIC + TLS transport.

```sh
cargo run -p rusm-bench --example cluster
```

Expected output:

```
[tokyo] connected to: ["london"]
[tokyo] 'greeter' lives on node: "london"
   [london] greeter received: hello from tokyo!
[tokyo] london is running 1 process(es)
```

## What to take away

- **Mutual TLS, closed by default.** This demo shares one `Identity::generate()`
  certificate across both nodes — every link is mutually authenticated, so a peer
  without the certificate can't complete the handshake. For production, prefer a
  `ClusterCa`: `ca.issue("node")` gives each node its own key and a CA-signed cert,
  so a compromised node can be revoked without re-keying the cluster.
- **Nodes have names and their own runtime.** `ClusterNode::bind("london", …)`
  wraps a normal `rusm_otp::Runtime` with a QUIC endpoint.
- **The global registry hides location.** `london.register_global("greeter", pid)`
  publishes a name cluster-wide; `tokyo.send_global("greeter", …)` reaches it
  without tokyo ever knowing which node it's on.
- **Live attach.** `tokyo.remote_pids("london")` lists what a peer is running —
  the primitive behind attaching to a live node.

## Adapt it

- Register more processes on london and message them all by name from tokyo.
- Add a third node and `connect` it to both — a fully-connected cluster gossips
  every global registration to every peer.
- Swap `send_global` for `tokyo.send("london", "greeter", …)` to address a peer
  node explicitly instead of by global name.

See [`cluster_fanout`](../../benchmarks/cluster_fanout/) for a throughput/latency benchmark of
the same transport.
