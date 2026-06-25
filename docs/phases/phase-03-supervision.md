# Phase 3 — Links, monitors, supervision

Let it crash — Phase 3 makes failure a first-class event: processes fail loudly, failures propagate along links, and supervisors turn a crash into a restart at ~285k restarts/sec.

## Why this phase

The point of isolation is that a failure stays *contained and visible*. Contained: a crash in one process doesn't corrupt another's memory. Visible: the processes that need to know, know — immediately, with a reason. Phase 3 delivers both.

Without links and monitors, a crashed process is an invisible hole. With them, every exit — normal or abnormal — can trigger exactly the right response in the right process. The supervisor pattern is built entirely on top: it's just a process that receives exits and decides to restart or escalate.

## What shipped

1. **Exit reasons** — `ExitReason::{Normal, Killed, Crashed, NoProc}` with `is_abnormal()`. Crash detection rides on `std::thread::panicking()` in the Drop guard from Phase 1 — no `catch_unwind`, zero cost on the happy path.
2. **`link` / `unlink`** — bidirectional. When a linked process exits abnormally, the signal propagates to its peers.
3. **`spawn_link(parent, body)`** — spawn already linked, atomically. No window where the child can die before the link exists.
4. **`monitor(watcher, target) -> MonitorRef`** — one-directional, non-fatal: the watcher receives `Received::Down { reference, pid, reason }` and decides what to do.
5. **`set_trap_exit(pid, true)`** — converts incoming exit signals into `Received::Exit { from, reason }` mailbox messages instead of killing the receiver. This is what lets a supervisor survive its children crashing.
6. **Exit cascades** — `exit(pid, reason)` propagates along links with a staged reason, tearing down a linked subtree exactly as Erlang's BEAM does.
7. **Fault-recovery engine** (`rusm-bench`) — a crash-and-restart loop reporting real restarts/sec.

## Design highlights

- **No `catch_unwind`.** Crash detection reuses the `ProcessGuard::drop` already present from Phase 1 — `std::thread::panicking()` is checked there. Failure capture costs nothing on the happy path, and there's no per-call overhead.
- **Signals reuse the mailbox.** `Down` and `Exit` are variants of the same `Received` stream established in Phase 2. One ordered queue, no separate signal plumbing, no reordering between messages and signals.
- **`trap_exit` is the only supervisor primitive needed.** The entire supervision pattern reduces to: `set_trap_exit(true)` on the supervisor, `spawn_link` for its children, and a receive loop that checks `reason.is_abnormal()`. No separate supervisor process type, no framework.
- **Atomic `spawn_link`.** The link is established before the child's first poll. A child that crashes immediately is still caught.

## What this unlocks

The OTP fault-tolerance model is now real. A supervisor process can watch any number of children, restart them on crash, and escalate if restarts fail too fast. Build one-for-one, one-for-all, and rest-for-one trees entirely from these primitives.

Every Wasm guest in Phase 7 onward benefits from this automatically — a crashing Wasm instance is `ExitReason::Crashed`, propagated through the same link/monitor machinery.

## Try it

```sh
cargo run -p rusm-bench -- run fault-recovery 5   # ~285k restarts/sec
```

```rust
// Inside a supervisor body — trap exits, spawn linked children, restart on crash
runtime.set_trap_exit(supervisor, true);
let child = runtime.spawn_link(supervisor, body);
// ... in the supervisor's receive loop:
if let Received::Exit { from, reason } = ctx.recv().await {
    if reason.is_abnormal() { /* restart `from` */ }
}
```

## Status

Phase complete. ~285k restarts/sec. Fault-recovery is live in the dashboard. The Wasm-free invariant holds: no `wasmtime` dependency anywhere under `rusm-otp`.

---

*Next: [Phase 4](./phase-04-management.md) — process management: named registry, timers, and graceful shutdown — the ergonomic layer that makes a process system livable.*
