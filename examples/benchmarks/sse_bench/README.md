# `sse_bench` — Server-Sent Events stress benchmark

Holds many concurrent `text/event-stream` connections against a WASM component that
streams events as fast as each client drains them, then reports total events/sec,
concurrent streams held, and stream-setup latency. This is the **"many long-lived
streaming responses"** story — exactly where a NATS-lattice host tends to wobble.
Each stream is one component instance; a dropped client tears down only its own
instance (the host write fails → the guest stream cancels), never the others or the
listener.

```sh
cargo run --release -p rusm-bench --example sse_bench -- [seconds] [streams]
```

Representative output (loopback, everyday machine load):

```
SSE stress: 128 concurrent event streams, 4s

streams held:  128/128   (each its own component instance)
events:        1496948/sec total   (11695/sec per stream)
stream setup:  p50 10583.3µs  p99 17948.5µs   (connect → first event)
```

## Reading it

- **`streams held: 128/128`** is the headline: 128 long-lived streaming responses held
  open and flowing, none dropped. SSE needs no special transport — it's a `wasi:http`
  component that sets `Content-Type: text/event-stream` and writes `data:` frames over
  time; the existing instance-per-request `HttpServer` already runs the handler in its
  own task and flushes each chunk the instant the guest yields it (`wasi:http` body
  back-pressure throttles the guest to the socket, so an idle stream costs ~nothing).
- **events/sec** is sustained throughput across all streams; per-stream is the fan-out.
- **stream setup** is connect → first event, dominated here by 128 *simultaneous* cold
  instantiations racing for the pooled allocator — it falls sharply with fewer
  concurrent opens, and steady-state event flow is unaffected.
- The event producer is a sandboxed WASM component (`sse-firehose`); the host only
  moves bytes. The `rusm-otp` core never sees hyper or `wasi:http`.
