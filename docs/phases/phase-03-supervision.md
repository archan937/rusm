# Phase 3 — links, monitors, supervision

**Goal:** fault tolerance the Erlang way — processes fail loudly, failures
propagate along links, and supervisors turn a crash into a restart. "Let it
crash." **Graduates:** the **fault-recovery** scenario to live data.

## Why this matters

The point of isolation is that a failure stays contained *and visible*. Links and
monitors are how one process learns another died; `trap_exit` is how a supervisor
turns that signal into a restart instead of dying itself.

## What we built (TDD throughout)

1. **Exit reasons** — `ExitReason::{Normal, Killed, Crashed, NoProc}` with
   `is_abnormal()`. The reason is captured at teardown: the `ProcessGuard` checks
   `std::thread::panicking()` in its `Drop`, so a panicking body is recorded as
   `Crashed` — **no `catch_unwind`, no per-call cost**.
2. **`link` / `unlink`** — bidirectional. When a linked process exits abnormally,
   the signal propagates to its peers.
3. **`spawn_link(parent, body)`** — spawn already linked, atomically (no window
   where the child can die before the link exists).
4. **`monitor(watcher, target) -> MonitorRef`** — one-directional, non-fatal: the
   watcher just receives a `Received::Down { reference, pid, reason }`.
5. **`set_trap_exit(pid, true)`** — converts incoming exit signals into
   `Received::Exit { from, reason }` mailbox messages instead of killing the
   receiver. This is what lets a supervisor survive its children.
6. **Exit cascades** — `exit(pid, reason)` propagates along links with a staged
   reason, so a crash can tear down a linked subtree exactly like the BEAM.
7. **Fault-recovery engine** (`rusm-bench`) — crash-and-restart loop reporting
   real restarts/sec (~285k/sec).

## How a developer uses it

```rust
runtime.set_trap_exit(supervisor, true);          // survive child exits
let child = runtime.spawn_link(supervisor, body);  // linked at birth
// ... supervisor's body:
if let Received::Exit { from, reason } = ctx.recv().await {
    if reason.is_abnormal() { /* restart `from` */ }
}
```

## Design notes — why it's cheap

- **No `catch_unwind`.** Crash detection rides on `thread::panicking()` in the
  Drop guard already present from [Phase 1](./phase-01-process-core.md) — failure
  capture costs nothing on the happy path.
- **Signals reuse the mailbox.** `Down`/`Exit` are variants of the same
  `Received` stream from [Phase 2](./phase-02-messaging.md) — one ordered queue,
  no separate signal plumbing.

## Concepts introduced

- **Links, monitors, supervision, cascades** — see
  [links & supervision](/deep-dive/links-and-supervision).

## Play with it

```sh
cargo run -p rusm-bench -- run fault-recovery 5   # ~285k restarts/sec
```

## Verification

`cargo test -p rusm-otp` green (link cascade, monitor Down, trap_exit, crash →
`Crashed`, kill → `Killed`, NoProc); fault-recovery live in the dashboard.

## Next

[Phase 4](./phase-04-management.md): **process management** — a named registry,
timers, and graceful shutdown.
