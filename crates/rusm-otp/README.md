# rusm-otp

> The Wasm-free Erlang/OTP core of RUSM — real lightweight processes, message passing, supervision, and connectivity, in pure Rust on Tokio.

`rusm-otp` is the heart of [RUSM](https://github.com/archan937/rusm): a from-scratch,
BEAM-inspired actor runtime built on Tokio, with **no dependency on WebAssembly** (it's
usable entirely standalone). Each process is a Tokio task scheduled M:N over a few OS
threads; the goal is hundreds of thousands of spawns per second.

## What it gives you

- **Processes & scheduling** — `spawn`, abort-based lifecycle, a sharded-`DashMap` process table.
- **Mailboxes & messaging** — per-process mailbox, `send`/`receive`, selective receive, `receive … after` timeouts.
- **Links, monitors, supervision** — exit reasons, `link`/`monitor`/`trap_exit`/`spawn_link`/`exit`, exit cascades, supervisors with windowed restart intensity.
- **Process management** — a named registry, **process groups** (Erlang `pg`: `register_tag`/`whereis_tag`/`kill_tag` — tag many processes, terminate the group), timers (`send_after`/`cancel`), graceful `shutdown`, introspection (`list`/`info`/`set_label`).
- **Connectivity** — TCP (`listen`/`connect`, one process per connection) and back-pressured byte streams (`Received::Stream`).

Exit signals ride the mailbox (a `Received` enum) and kill rides a `futures` abort handle —
one channel per process. The same core powers the `rusm-cluster` distributed transport and,
via `rusm-wasm`, hosts sandboxed WebAssembly processes.

```rust
use rusm_otp::Runtime;

let rt = Runtime::new();
let pid = rt.spawn(|mut ctx| async move {
    while let Some(msg) = ctx.recv().await.message() { /* handle */ }
});
rt.send(pid, b"hello".to_vec());
```

Part of [RUSM](https://github.com/archan937/rusm). See the
[repo README](https://github.com/archan937/rusm#readme) and the
[architecture docs](https://github.com/archan937/rusm/blob/main/docs/about/architecture.md).
