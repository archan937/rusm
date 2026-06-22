# Phase 5 — connectivity: TCP

**Goal:** let processes serve real traffic — `listen`/`connect`, with **one
process per connection** so a slow or crashing client can't touch the others.
**Graduates:** the **connection-storm** scenario to live data. (TLS folds into the
[Phase 9](/about/roadmap) secure cluster transport.)

## Why this matters

The actor model's payoff for networking is that a connection is *just a process*.
Accept a socket → spawn a process to own it. Isolation, supervision, and cheap
spawning all apply to connections for free, exactly as the BEAM does it.

## What we built (TDD throughout)

1. **`listen(addr, handler) -> (SocketAddr, ProcessHandle)`** — binds a
   `TcpListener` and runs an acceptor process; every accepted socket is spawned as
   its **own** process running `handler(ctx, stream)`. Returns the bound address
   (handy with port 0) and a handle to the acceptor — **kill it to stop
   listening** (dropping the listener closes the port).
2. **`connect(addr) -> io::Result<TcpStream>`** — opens an outbound connection.
3. **Connection-storm engine** (`rusm-bench`) — ramp-and-hold load reporting real
   connections/sec and peak concurrency.

## How a developer uses it

```rust
let (addr, acceptor) = runtime.listen("127.0.0.1:0", |ctx, mut stream| async move {
    // This closure runs as its own isolated process, one per connection.
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    stream.write_all(&buf[..n]).await.unwrap();      // echo
}).await?;
// ... later:
acceptor.kill();                                     // stop listening
```

## Design notes — honest about the ceiling

- **The OS is the limit, not RUSM.** RUSM mints processes far faster than any TCP
  stack hands out sockets, so sustained connection rate is bounded by the kernel's
  handshake/ephemeral-port/`TIME_WAIT` budget — the same ceiling *any* runtime
  hits. We measure it honestly (~6–16k/s on loopback) rather than inflate it.
- **`SO_LINGER(0)` to avoid `TIME_WAIT` exhaustion.** An early version showed a
  291/s sawtooth: client active-close piled up `TIME_WAIT` sockets. Closing with a
  reset (via `socket2::SockRef`) frees them immediately, giving a *sustained* rate
  instead of a collapsing one.
- **Low-concurrency ramp-and-hold.** Flooding with parallel connectors just
  exploded latency (the OS serializes handshakes anyway); a steady ramp measures
  the true ceiling. The fd limit is raised at startup via the `rlimit` crate.

## Concepts introduced

- **Process-per-connection** — a connection is just another isolated process; see
  [the process model](/deep-dive/wasm-instance-as-process).

## Play with it

```sh
cargo run -p rusm-bench -- run connection-storm 5   # real sustained connections
```

## Verification

`cargo test -p rusm-otp` green (listen/echo round-trip, port-0 bind, kill stops
listening, connect failure); connection-storm live in the dashboard. This
completes the Wasm-free OTP core — `rusm-otp` has **zero `wasmtime` dependency**.

## Next

[Phase 6](./phase-06-wasm-backend.md): **Wasmtime as the process backend** — each
process becomes a sandboxed Wasm instance.
