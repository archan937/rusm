# Build a stateful service

Serving instances are ephemeral — they forget everything between requests. When you need
state that **survives and is shared** (a counter, a cache, a registry, a pub/sub broker, the
"current value" of something), put it in a **resident service**: one long-lived component
that the node boot-spawns and supervises, holding its state in memory and answering callers
over messages.

It's the same "export functions" shape as any service — the difference is one line in the
manifest (`resident = true`) and that **module/struct scope is now durable state** for the
life of the process.

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

A resident service **registers itself by name** so any component can find it without spawning
a new one — `whereis("counter")` → its pid → call it (the typed client wraps this; see
[Call another component](/build-an-app/call-another-component)). One instance, shared by all
callers, single-threaded over its mailbox — so its state needs no locks; messages are handled
one at a time.

## What you need to know

- **Supervised.** `resident = true` puts it under the node's supervisor: if it crashes it's
  restarted (bounded by restart-intensity). State in memory is lost on restart by design —
  persist anything that must survive a crash to the node `store` (`kv`).
- **One writer, no races.** Because a service processes its mailbox one message at a time, its
  in-memory state is a single-threaded island — no mutexes, no data races.
- **This is where shared state lives.** Serving handlers (HTTP/WS/SSE) and workers are
  ephemeral; when they need shared or durable state, they call a resident service or use `kv`
  — never keep it in the ephemeral instance. See the worker-vs-service table in
  [Run one-off work](/build-an-app/run-one-off-work).
