# Concept — fibers & "write blocking code, get async"

The headline RUSM (and Lunatic) property: a guest writes ordinary **blocking**
code, but the runtime turns blocking calls into async suspensions. Guests never
write `async`.

## How it works

Wasmtime's **async support** runs each instance on its own **fiber** (a separate
stack). When a guest calls a host function that is `async` on the Rust side —
say `receive()` waiting on an empty mailbox — the host `await`s, and Wasmtime
**suspends the entire guest call stack** by switching off the fiber. The Tokio
task yields; the OS thread runs another process. When the await resolves,
Wasmtime switches the fiber back in and the guest call returns, none the wiser.

## Why this matters

- **Simpler guests.** No `async`/`await` noise; you can call blocking C libraries
  compiled to Wasm and the runtime still won't block a thread.
- **Massive concurrency.** Millions of "blocked" processes are just parked tasks.

## Relation to Lunatic's "custom stack switching"

Lunatic cites a libfringe-inspired stack switcher. Wasmtime's fiber support is
the same idea — stack switching — but battle-tested and safe, so RUSM uses it
first (a hand-rolled version is a Phase 10 stretch). See also
[epoch preemption](/deep-dive/epoch-preemption) for *fair* scheduling on top of this.

> Shipped in Phase 6 (Wasmtime backend).
