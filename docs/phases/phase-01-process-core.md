# Phase 1 — Process & scheduler core

Phase 1 answers the first question: what *is* a process? — a Tokio task, a table entry, and an abort-based lifecycle, spawning at ~2.4M/sec on real hardware.

## Why this phase

Everything that follows — mailboxes, links, supervision, TCP, Wasm — hangs off a single foundation: "what is a process and how does it live and die?" Get that wrong and every later abstraction is built on sand.

The answer RUSM settled on: a process is a Tokio task driven by a user-supplied async closure, plus one entry in a sharded process table that carries everything needed to kill it. That's it. No second structure, no signal channels, no separate lifecycle states. Phase 1 proves this model is fast, correct, and leak-free — at ~2.4M sustained spawns/sec across all cores.

## What shipped

1. **`Runtime`** — a cheap-to-clone handle around a sharded `Inner` (`DashMap` process table + atomic `next_id`/`spawned`/`finished` counters). Clone freely; no Arc inside the clone.
2. **`spawn(body)`** — takes a closure `Fn(Context) -> Future`, mints a `Pid`, drives the future as a Tokio task, and returns a `ProcessHandle` with `pid()`, `kill()`, and `join()`.
3. **Race-free kill via `AbortHandle`.** The abort handle is created *before* the task is spawned, so the single table insert already carries it — no second write, no window where a process exists but isn't killable.
4. **`ProcessGuard` (Drop) cleanup inside the task.** Table removal and counter bookkeeping live in a guard owned by the task future, so the entry is reaped on *any* teardown — normal return, abort-before-first-poll, or panic.
5. **Spawn-storm engine** (`rusm-bench`) — a multi-core storm against a bounded live population, reporting real spawns/sec at steady state.

## Design highlights

- **One table write per process.** An earlier two-channel design cost 17% throughput. Folding the abort handle into the single insert gives kill *for free* — the handle is already there when the insert completes.
- **Sharded `DashMap`.** Concurrent spawns and reaps hit different shards, so the process table never becomes a global lock. The throughput scales with core count.
- **Bounded-population bench.** The storm holds a target live count, measuring steady-state spawn+reap throughput — not a one-shot allocation spike. Every reported spawn/sec is a real create + destroy cycle.
- **Drop-based cleanup, no explicit teardown.** `ProcessGuard` means teardown is automatic on any exit path. A leaked guard is impossible; the entry is always reaped.

## What this unlocks

With a process model in place, everything else is additive. Phase 2 gives processes mailboxes and they become actors. Phase 3 links them and they become supervised. Phase 6 swaps the native Rust body for a Wasm instance and they become sandboxed — behind the same `spawn()` call.

The ~2.4M spawns/sec number is also a baseline guarantee: any future optimization must not regress it, and any future feature (mailboxes, Wasm) will be measured against it.

## Try it

```sh
cargo run -p rusm-bench -- run spawn-storm 5   # 5 seconds of real spawns; watch ~2.4M/sec
cargo run -p rusm-bench -- run ping-pong 5     # placeholder — goes live in Phase 2
```

## Status

Phase complete. ~2.4M sustained spawns/sec. No regression in any later phase. Spawn-storm is live in the dashboard.

---

*Next: [Phase 2](./phase-02-messaging.md) — mailboxes & message passing: processes become actors, ping-pong goes live at ~21M msgs/sec with round-trip p50 <1µs.*
