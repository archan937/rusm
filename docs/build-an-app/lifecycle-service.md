# Lifecycle — Service component (non-serving, resident)

A long-lived, **stateful** process — a `#[rusm_rs::service]` holding state (a counter, a
cache, a pub/sub hub), reached via `whereis` + `call`/`send`. **This is where "resident
vs per-call" lives — never in the serving tier.** See the
[overview](/build-an-app/component-lifecycle) for the shared two-domain model and failure
vocabulary.

## Shape (what you write)

*A complete service wired into a real app is the `store` component of the todo-board examples — [TypeScript](https://github.com/archan937/rusm/tree/main/examples/typescript/components/store) · [Rust](https://github.com/archan937/rusm/tree/main/examples/rust/components/store) · [Go](https://github.com/archan937/rusm/tree/main/examples/go/components/store).*

::: code-group

```ts [TypeScript]
// counter.ts — each exported function is a call; module scope holds the state.
let count = 0;
export function bump(by: number): number { count += by; return count; }
export function total(): number { return count; }
```

```rust [Rust]
#[rusm_rs::service]
pub mod counter {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0); // state the loop owns
    // the macro writes the receive → dispatch → reply loop
    pub fn bump(by: u64) -> u64 { COUNT.fetch_add(by, Ordering::Relaxed) + by }
    pub fn total() -> u64 { COUNT.load(Ordering::Relaxed) }
}
```

```go [Go]
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

var count int // state the loop owns

func run() {
	svc := rusm.NewService()
	svc.Handle("bump", rusm.Fn1(func(by int) (int, error) { count += by; return count, nil }))
	svc.Handle("total", rusm.Fn0(func() (int, error) { return count, nil }))
	svc.Serve() // the dispatch loop: receive → call → reply
}
```

:::

Declared as a `[components.<name>]` entry (with `resident = true` to be boot-spawned and
supervised), spawned when the node starts, and addressed by name. A sibling calls it through the generated
typed client — `spawn<Counter>("counter")` then `await counter.bump(1)` (or, in Go,
`rusm.Call[int](counter, "bump", 1)`) — and the cross-process round-trip reads like a
local call (Rust, TypeScript, and Go interoperate over one wire).

## Platform owns / you write

- **Platform owns:** spawning it at node start, the mailbox, **reply routing** for each
  `call` (matching a reply to its request), the **supervisor + restart policy**, and
  parking the fiber on `receive` between requests.
- **You write:** the handler functions and the state they own (the macro writes the
  `serve()` dispatch loop).

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Normal run** | blocks the fiber on `receive` between requests; routes each `call`'s reply back to its caller | handles calls/casts, mutates its own state | serves until shutdown |
| **Graceful shutdown** | signals shutdown; lets the loop drain | finishes in-flight work, returns | clean stop |
| **Crash (trap)** | Crashed → the **supervisor restarts it with fresh state** (per intensity); monitors get `Down`, links get an exit signal | the `panic!` / `.unwrap()` | restarted (state reset); a sibling's in-flight `call` **fails** — its reply ref is never answered, so the caller's `receive`-with-timeout returns an error rather than hanging |
| **Memory crash (OOM)** | the `StoreLimiter` cap trips a trap → Crashed → restart | exceeded `max-memory-mb` | restarted; state reset |
| **Restart storm** | restart-intensity window exceeded → the supervisor **escalates** (gives up) rather than crash-loop | (a persistent bug) | the failure surfaces upward |
| **Kill** (explicit or via a link) | abort handle fires; the signal rides the mailbox | — | reclaimed (restarted if supervised) |

## Notes

- **State resets on restart — by design.** "Let it crash": a supervised service comes
  back clean rather than limping on corrupt state. If state must survive a restart, put
  it in durable `kv`, not the process heap.
- **A call to a dead service fails, it doesn't hang.** The reply ref is never answered,
  so a caller using `call`-with-timeout gets a clear error and can react — back-pressure
  for failure, not a silent stall.
- **The home for shared state.** [HTTP](/build-an-app/lifecycle-http),
  [SSE](/build-an-app/lifecycle-sse), and [WebSocket](/build-an-app/lifecycle-websocket) components are
  stateless and disposable; anything they need to share (a cache, a broker, a session
  map) lives in a service like this and is reached by message.

Prev: [Worker component](/build-an-app/lifecycle-worker) · Overview: [Component lifecycles](/build-an-app/component-lifecycle)
