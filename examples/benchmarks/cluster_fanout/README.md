# `cluster_fanout` — cross-node throughput & latency benchmark

Measures the **real** QUIC + TLS cross-node messaging performance of a RUSM
cluster: one hub node fanning messages out to worker nodes that bounce them back.

```sh
# release, for real numbers:  [seconds] [worker-nodes]
cargo run --release -p rusm-bench --example cluster_fanout -- 5 4
```

Representative output (loopback, everyday machine load):

```
cluster fan-out: 4 worker nodes, 32 senders (QUIC+TLS loopback)

unloaded round-trip latency: p50 39.3µs  p99 111.5µs
saturation round-trips:      1381073  (276163/sec)
saturation cross-node msgs:  2762146  (552325/sec)
```

## How it measures (and why two numbers)

Every worker runs an `echo` process; each message carries
`[8-byte send-time][reply-process-name]`, so one echo serves both phases:

1. **Unloaded latency** — one round-trip in flight at a time against a `probe`
   process. This is the transport's true round-trip time (hub → worker → hub, two
   cross-node hops), with no queueing.
2. **Saturation throughput** — a pool of senders floods the links; a `collector`
   counts completed round-trips.

They're measured separately on purpose: under saturation, "latency" is dominated
by queue depth, not the wire — reporting one number for both would mislead.

> A round-trip is **two** cross-node hops, so cross-node messages/sec is twice the
> round-trips/sec.

## Notes

- All nodes run in one process on loopback — a faithful stand-in for separate
  machines that isolates the transport from real network variance.
- Numbers scale with free CPU and the worker/sender counts; pass larger arguments
  to push harder.
