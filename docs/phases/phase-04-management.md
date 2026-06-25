# Phase 4 — Process management

Phase 4 is the ergonomic layer that makes a process system livable: stable names, delayed messages, and a clean way to stop everything at once.

## Why this phase

Phases 1–3 delivered a real actor model — processes spawn, message each other, crash, and recover. But raw pids are ephemeral. Pass one around long enough and it may point to a dead process. A real system needs stable names you can look up by string, timers that fire without dedicating a process, and a shutdown path that doesn't leave lingering tasks.

These aren't glamorous features. They're the difference between "this works in a demo" and "this is something you can build on." Phase 4 closes that gap.

## What shipped

1. **Named registry** — a sharded `DashMap` (`name → pid`) making registration and lookup concurrent and lock-free in the common case: `register(name, pid)`, `whereis(name)`, `unregister(name)`, and `send_named(name, msg)` — resolve and deliver in one step. Names are reaped automatically when the owning process exits; a dead name never resolves to a stale pid.
2. **Timers — `send_after(pid, delay, msg) -> TimerRef`** — delivers a message after a delay, on Tokio's hierarchical timer wheel. Thousands of pending timers cost almost nothing. `TimerRef::cancel()` stops a not-yet-fired timer.
3. **Graceful `shutdown() -> usize`** — kills every live process and returns the count. The node stops cleanly.

## Design highlights

- **Registry sharding matches the process table.** Naming never becomes a global lock — the same concurrency model as Phase 1's `DashMap` process table, applied to names.
- **Timers ride Tokio's wheel, not a task-per-timer.** Pending timers are nearly free, so `send_after` scales with process count — not with "how many timers are live."
- **Self-cleaning names.** Deregistration is part of the same Drop path that reaps the table entry from Phase 1. There is no stale-name window to manage by hand, and no `unregister_on_exit` call to remember.
- **`send_named` is atomic.** Resolve + deliver happen in a single registry operation. No TOCTOU between finding the pid and sending to it.

## What this unlocks

With the registry, processes publish stable service names and clients look them up — no pid threading through the whole application. `send_named("logger", msg)` works whether the logger was spawned 100ms ago or 10 minutes ago.

Timers enable timeout patterns, periodic heartbeats, and deadline-based cancellation without spinning up a dedicated timer process. `send_after` + `recv_match` is the building block for `receive_timeout` — Erlang's `receive … after N → …` — which arrives in the Wasm ABI in Phase 7.

## Try it

```sh
cargo test -p rusm-otp registry   # register / whereis / auto-reap on process exit
cargo test -p rusm-otp timer      # send_after fires; cancel stops it
```

## Status

Phase complete. Registry is concurrent, lock-free, self-cleaning. Timers use zero per-timer overhead. No new dashboard scenario; this phase rounds out the single-node API surface.

---

*Next: [Phase 5](./phase-05-tcp.md) — connectivity: TCP listen/connect, one process per connection, connection-storm goes live.*
