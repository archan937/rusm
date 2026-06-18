# `ws_bench` — WebSocket stress benchmark

Holds many concurrent WebSocket connections and hammers each with echo round-trips.
Two servers make the sandbox cost explicit: the **component path** (every connection
is a sandboxed WASM **component process** — the real serving path) and a **host echo**
(no Wasm — the transport ceiling). The headline is the **stability under load** proof:
each connection is an isolated supervised pair (a reader pump + a writer process + the
component), so one dropping never touches the others or the listener.

```sh
cargo run --release -p rusm-bench --example ws_bench -- [seconds] [connections]
```

Representative output (loopback, everyday machine load):

```
WebSocket stress: 128 concurrent connections, 4s each

WASM component per connection (real serving path):
  connections held: 128/128   (each an isolated supervised pair)
  echo round-trips:  192059/sec   p50 646.8µs  p99 1162.0µs

host echo (no Wasm, transport ceiling):
  connections held: 128/128   (each an isolated supervised pair)
  echo round-trips:  183714/sec   p50 682.1µs  p99 1147.4µs

component vs host transport: 1.05x throughput, -35.3µs p50  (the sandbox cost per round-trip)
```

## Reading it

- **`connections held: 128/128`** is the headline — no flakiness, no cascading drops.
  RUSM has no lattice or broker between the socket and the handler (unlike a NATS-based
  host), so there's far less to wedge.
- **The component path matches the bare transport.** The per-message cost is one
  `writer → component → writer` mailbox hop, which is ~free next to the socket I/O — so
  serving WebSockets *through a sandboxed WASM component* costs essentially nothing over
  raw hyper + tungstenite. (Run-to-run the two trade places inside noise.)
- The upgrade + WebSocket protocol run host-side (hyper + `tokio-tungstenite`); after
  the upgrade, each inbound frame becomes one mailbox message to the component, and its
  replies flow back through a Wasm-free **writer process** that owns the socket sink —
  clean actor separation, the `rusm-otp` core untouched by Wasm.
- `p50`/`p99` are round-trip latency under N-way concurrency; throughput scales with
  free CPU and connection count.
