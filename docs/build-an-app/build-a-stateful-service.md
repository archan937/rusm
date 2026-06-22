# Build a stateful service

Serving instances are ephemeral — they forget everything between requests. When you need
state that **survives and is shared** (a counter, a cache, a registry, a pub/sub broker, the
"current value" of something), put it in a **resident service**: one long-lived component
that the node boot-spawns and supervises, holding its state in memory and answering callers
over messages.

It's the same "export functions" shape as any service — the difference is one line in the
manifest (`resident = true`) and that **module/struct scope is now this instance's live,
in-memory state** for the life of the process.

::: code-group

```ts [TypeScript]
// components/counter/index.ts — module scope is the state; each export is a call.
let count = 0;
export function bump(by: number): number { count += by; return count; }
export function total(): number { return count; }

export type Counter = typeof import(".");
```

```rust [Rust]
// components/counter/src/lib.rs
#[rusm_rs::service]
pub mod counter {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);          // state the loop owns
    pub fn bump(by: u64) -> u64 { COUNT.fetch_add(by, Ordering::Relaxed) + by }
    pub fn total() -> u64 { COUNT.load(Ordering::Relaxed) }
}
```

```go [Go]
// components/counter/main.go
func run() {
	count := 0 // closed over by the handlers — this instance's state
	svc := rusm.NewService()
	svc.Handle("bump", rusm.Fn1(func(by int) (int, error) { count += by; return count, nil }))
	svc.Handle("total", rusm.Fn0(func() (int, error) { return count, nil }))
	svc.Serve()
}
```

:::

Mark it **resident** in `rusm.toml` so the node boot-spawns and supervises it:

```toml
[components.counter]
capability = "sandboxed"
resident = true          # boot-spawned at startup + supervised (auto-restart on crash)
```

A resident is **one** instance, processing its mailbox one message at a time — so its state
needs no locks. Two things to get right: **where the state lives**, and **how callers reach
that one instance**.

## Where the state lives

- **In memory** (the module / struct / closure scope above) is this instance's working state.
  It's fast and lock-free, but it's **gone on restart** — the supervisor restarts a crashed
  service with a clean slate, by design.
- **Durable `kv`** is for state that must survive a restart, or be shared with ephemeral
  serving handlers that can't hold a client. The service composes the node's
  [`kv`](/build-an-app/url-shortener) store. This is what the runnable example apps do: the
  `store` service and the HTTP `api` share one todo list through `kv`, so it stays consistent
  and durable no matter which instance serves a call.

For state shared across the whole app, reach for **`kv`** — it's the simplest correct answer
in every language.

## How callers reach it

A resident is **one** instance, so to talk to it a caller needs its pid — **not** `spawn`,
which would start a *fresh* instance. The service **claims a name** in its entry; a caller
**looks that name up** and calls the pid. This is ordinary app code you write — two lines on
each side:

::: code-group

```rust [Rust]
// Service entry: claim a name, then run the dispatch loop.
rusm_rs::register("counter");
counter::serve();

// Caller: reach the running instance by pid — a typed call, with the reply.
let c = counter::Client::connect(rusm_rs::whereis("counter").unwrap());
let total = c.bump(1).unwrap();
```

```go [Go]
// Service: claim a name before Serve().
rusm.Register("counter")
svc.Serve()

// Caller: look it up and call by pid (Call takes any pid, returns the reply).
pid, _ := rusm.Whereis("counter")
total, _ := rusm.Call[int](pid, "bump", 1)
```

:::

**TypeScript** has no typed call to an *existing* pid yet — `spawn<T>(name)` only starts a fresh
instance, a parity gap with Rust's `Client::connect(pid)` and Go's `Call(pid, …)`. So from TS,
share a resident's state through [`kv`](/build-an-app/url-shortener), the cross-language answer
the example apps use. (Dropping to a raw `Process.send` with a hand-built request envelope does
reach a resident, but that's the platform's internal wire protocol, not application code —
prefer `kv`.)

## What you need to know

- **Supervised.** `resident = true` puts it under the node's supervisor — a crash restarts it
  (bounded by restart-intensity), with in-memory state reset. Persist anything that must
  survive to `kv`.
- **One writer, no races.** A service handles its mailbox one message at a time, so its
  in-memory state is a single-threaded island — no mutexes, no data races.
- **Ephemeral callers use `kv`.** Serving handlers (HTTP/WS/SSE) and workers are short-lived;
  when they need shared or durable state, the robust cross-language answer is `kv` (the example
  apps' approach), not in-memory state in another process. See the worker-vs-service table in
  [Run one-off work](/build-an-app/run-one-off-work).
