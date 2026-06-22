# Phase 4 — process management

**Goal:** the everyday management surface that makes a process system usable — a
named registry (look processes up by name, not pid), timers (`send_after`), and a
graceful, system-wide `shutdown`. No new scenario graduates here; it rounds out
the single-node API.

## Why this matters

Real systems don't pass raw pids around forever — they want stable names, delayed
work, and a clean way to stop. These are the small ergonomics that turn the actor
core into something you can build on.

## What we built (TDD throughout)

1. **Named registry** — a sharded `DashMap` (`name → pid`), so registration and
   lookup are concurrent and lock-free in the common case:
   - `register(name, pid) -> bool` (fails if the name is taken),
   - `whereis(name) -> Option<Pid>`,
   - `unregister(name) -> bool`,
   - `send_named(name, msg) -> bool` — resolve and deliver in one step.
   - Names are reaped automatically when their process exits (the `ProcessGuard`
     deregisters), so a dead name never resolves to a stale pid.
2. **Timers — `send_after(pid, delay, msg) -> TimerRef`** — delivers a message
   after `delay`, on **Tokio's hierarchical timer wheel**, so thousands of pending
   timers cost almost nothing. `TimerRef::cancel()` stops a not-yet-fired timer.
3. **Graceful `shutdown() -> usize`** — kills every live process and returns the
   count, so a node can stop cleanly.

## How a developer uses it

```rust
runtime.register("logger", logger_pid);
runtime.send_named("logger", b"hello".to_vec());     // by name, no pid needed

let t = runtime.send_after(pid, Duration::from_secs(5), b"tick".to_vec());
t.cancel();                                           // ... or let it fire

let stopped = runtime.shutdown();                     // clean teardown
```

## Design notes

- **Registry is sharded, like the process table** — naming never becomes a global
  lock, matching the rest of the runtime's concurrency story.
- **Timers ride Tokio's wheel**, not a task-per-timer — pending timers are nearly
  free, so timeouts scale with the process count.
- **Self-cleaning names** — deregistration is part of the same Drop path that
  reaps the table entry, so there is no stale-name window to manage by hand.

## Concepts introduced

No new headline concept — this is the management surface over the
[process model](/deep-dive/wasm-instance-as-process) and the
[mailbox](/deep-dive/message-passing). See the
[host ABI reference](/deep-dive/host-abi) for the full call list.

## Play with it

```sh
cargo test -p rusm-otp registry          # registry behaviour
cargo test -p rusm-otp timer             # send_after / cancel timing (bounded)
```

## Verification

`cargo test -p rusm-otp` green (register/whereis/unregister, name reaped on exit,
`send_named`, timer fires within tolerance, cancel, shutdown count); coverage ≥98%.

## Next

[Phase 5](./phase-05-tcp.md): **connectivity** — TCP `listen`/`connect`, one
process per connection.
