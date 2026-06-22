# Fibers & blocking→async

This is the headline RUSM (and Lunatic) property: **a guest writes ordinary blocking code,
and the runtime turns every blocking call into an async suspension.** Guests never write
`async`, never juggle futures, never color a function — yet a million "blocked" processes
cost almost nothing. Here's the trick that makes it work.

## How it works

Wasmtime's **async support** runs each instance on its own **fiber** — a separate call
stack. When a guest calls a host function that is `async` on the Rust side — say `receive()`
waiting on an empty mailbox — the host `await`s, and Wasmtime **suspends the guest's entire
call stack** by switching off the fiber. The Tokio task yields, the OS thread picks up
another process, and when the await resolves Wasmtime switches the fiber back in. The guest
call simply returns, none the wiser that it was ever parked.

## Why it matters

- **Simpler guests.** No `async`/`await` noise anywhere in your code — and you can call
  blocking C libraries compiled to Wasm without ever blocking a real thread.
- **Massive concurrency.** A "blocked" process is just a parked task, so millions of them
  fit on a few OS threads. This is what makes process-per-request and process-per-connection
  serving cheap.

## Relation to Lunatic's stack switching

Lunatic cites a libfringe-inspired custom stack switcher. Wasmtime's fiber support is the
same idea — stack switching — but battle-tested and memory-safe, so RUSM uses it first (a
hand-rolled version is a Phase 10 stretch). For *fair* scheduling on top of this — so a
guest that never makes a host call still can't hog a thread — see
[epoch preemption](/deep-dive/epoch-preemption).

> Shipped in Phase 6 (the Wasmtime backend).
