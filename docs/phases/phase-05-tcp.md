# Phase 5 — Connectivity: TCP

A connection is just a process — Phase 5 maps TCP sockets onto the actor model, giving every connection its own isolation, supervision, and crash recovery for free.

## Why this phase

The actor model's payoff for networking is not a clever abstraction — it's structural. Accept a socket, spawn a process to own it. That process has a mailbox, can be killed, can be supervised, and can crash without touching any other connection. The OS hands you a socket; RUSM hands you a process. Everything from Phase 3 applies immediately.

Without this, a process system is a closed universe. With it, RUSM processes speak to the outside world — and the outside world is handled with the same fault-tolerance model as everything else.

## What shipped

1. **`listen(addr, handler) -> (SocketAddr, ProcessHandle)`** — binds a `TcpListener` and runs an acceptor process. Every accepted socket is spawned as its own process running `handler(ctx, stream)`. Returns the bound address (handy with port 0) and a handle to the acceptor — kill it to stop listening, killing the port.
2. **`connect(addr) -> io::Result<TcpStream>`** — opens an outbound TCP connection.
3. **Connection-storm engine** (`rusm-bench`) — a ramp-and-hold load reporting real connections/sec and peak concurrency.

## Design highlights

- **The OS is the limit, not RUSM.** RUSM mints processes far faster than any TCP stack hands out sockets. Sustained connection rate is bounded by the kernel's handshake/ephemeral-port/`TIME_WAIT` budget — the same ceiling every runtime hits. RUSM measures it honestly (~6–16k/s on loopback) rather than inflating the number.
- **`SO_LINGER(0)` to avoid `TIME_WAIT` exhaustion.** An early version showed a 291/s sawtooth: client active-close piled up `TIME_WAIT` sockets. Closing with a reset via `socket2::SockRef` frees them immediately, giving a sustained rate instead of a collapsing one — a real production correctness fix.
- **Ramp-and-hold, not flood.** Flooding with parallel connectors exploded latency (the OS serializes handshakes anyway); a steady ramp measures the true ceiling. The fd limit is raised at startup via the `rlimit` crate.
- **Kill the acceptor, kill the port.** The acceptor process *owns* the `TcpListener`. Drop the handle → the process exits → the listener drops → the port closes. No separate lifecycle to manage.

## What this unlocks

Any network protocol can be served by spawning a handler process per connection: HTTP, WebSocket, custom binary protocols. Each connection is isolated, supervised, and killable. A misbehaving client crashes its process; other connections are unaffected.

This is the direct predecessor to Phase 9's cluster transport (QUIC instead of TCP, for per-stream HoL-block freedom) and Phase 11's HTTP/WS/SSE serving model — process-per-request and process-per-connection all the way down.

This phase also completes the Wasm-free OTP core: `rusm-otp` has **zero `wasmtime` dependency** and stands as a fully capable actor runtime on its own.

## Try it

```sh
cargo run -p rusm-bench -- run connection-storm 5   # real sustained connections; watch the rate plateau
```

## Status

Phase complete. Connection-storm is live in the dashboard. The Wasm-free invariant holds: `rusm-otp` carries no `wasmtime` dependency.

---

*Next: [Phase 6](./phase-06-wasm-backend.md) — Wasmtime as the process backend: swap a native Rust body for a sandboxed Wasm instance, behind the same `spawn()` call.*
