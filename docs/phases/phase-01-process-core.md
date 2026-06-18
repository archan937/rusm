# Phase 1 — process & scheduler core

**Goal:** the first real step of the actor model — a *process* that is one Tokio
task plus an entry in a process table, with an abort-based lifecycle. Native Rust
bodies for now; Wasmtime slots in as the backend at [Phase 6](./phase-06-wasm-backend.md).
**Graduates:** the **spawn-storm** scenario to live data.

## Why this first

Everything later — mailboxes, links, supervision, TCP, Wasm — hangs off "what is
a process and how does it live and die." Get the lifecycle right (spawn cheaply,
count it, kill it without leaks) and the rest is additive.

## What we built (TDD throughout)

1. **`Runtime`** — a cheap-to-clone handle around a sharded `Inner` (a
   `DashMap` process table + atomic `next_id`/`spawned`/`finished` counters).
2. **`spawn(body)`** — takes a closure `Fn(Context) -> Future`, mints a `Pid`,
   and drives the future as a Tokio task. Returns a `ProcessHandle` (`pid()`,
   `kill()`, `join()`).
3. **Race-free kill via `AbortHandle`.** The abort handle is created from the
   task *before* it is spawned, so the single table insert already carries it —
   no second write, no window where a process exists but isn't killable.
4. **`ProcessGuard` (Drop) cleanup, inside the task.** Table removal and counter
   bookkeeping live in a guard owned by the task future, so the entry is reaped on
   *any* teardown — normal return, abort-before-first-poll, or panic.
5. **Spawn-storm engine** (`rusm-bench`) — a multi-core spawn storm against a
   bounded live population, reporting real spawns/sec.

## Design notes — why it's fast

- **One `DashMap`, sharded** — concurrent spawns/reaps hit different shards, so
  the table isn't a global lock. ~2.4M sustained spawns/sec across all cores.
- **One table write per process.** An earlier two-channel design cost 17%
  throughput; folding the abort handle into the single insert gave kill *for free*.
- **Bounded population.** The storm holds a target live count so we measure
  steady-state spawn+reap throughput, not a one-shot allocation spike.

## Concepts introduced

- **A process = a task + a table entry** — see
  [the process model](../concepts/wasm-instance-as-process.md).
- **Abort-based lifecycle** — cooperative cancellation via Tokio's `AbortHandle`,
  with Drop-based cleanup so teardown can never leak an entry.

## Play with it

```sh
cargo run -p rusm-bench -- run spawn-storm 5      # 5 seconds of real spawns
cargo run -p rusm-bench -- run ping-pong 5        # 5 seconds of real round-trips
```

## Verification

`cargo test -p rusm-otp` green (spawn, kill, join, count, no-leak-on-abort);
spawn-storm runs real processes in the dashboard; coverage ≥98%.

## Next

[Phase 2](./phase-02-messaging.md): **mailboxes & message passing** — each
process gets an async mailbox, `send`/`recv`, and selective `recv_match`.
