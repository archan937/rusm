# Call another component

Components don't share memory — they talk by **messages**. But you rarely write raw
send/receive: you define a component that **exports functions** (a service), and call it
through a **typed client** that hides the spawn + send + receive behind an ordinary
`await`/function call. The two components stay isolated and can even be in different languages
(one JSON wire).

## 1. The component you call — export functions

::: code-group

```ts [TypeScript]
// components/calc/index.ts — each exported function is callable; module scope holds state.
export function add(a: number, b: number): number { return a + b; }
export function* countTo(n: number) { for (let i = 1; i <= n; i++) yield i; } // streaming

// Publish the contract — derived from the exports, so it never drifts.
export type Calc = typeof import(".");
```

```rust [Rust]
// components/calc/src/lib.rs — a #[service] mod becomes a dispatch loop + a typed Client.
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n } // streaming
}
```

```go [Go]
// components/calc/main.go — register typed handlers; a generic Call[R] client calls them.
func run() {
	svc := rusm.NewService()
	svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
	svc.HandleStream("countTo", func(req rusm.Request, out rusm.Sink) error { // streaming
		n, _ := rusm.Arg[int](req, 0)
		for i := 1; i <= n; i++ { out.Send(i) }
		return nil
	})
	svc.Serve()
}
```

:::

## 2. The caller — spawn and call

`spawn` starts the component by its `rusm.toml` name; the typed client turns each method into
a real cross-process message. A **generator/stream** handler is consumed with `for await` /
an iterator; a function passed as an argument becomes a **callback** that runs back in the
caller.

::: code-group

```ts [TypeScript]
import { spawn } from "rusm-ts";
import type { Calc } from "../calc"; // type-only — the contract, not the code

export default async function () {
  const calc = spawn<Calc>("calc");          // spawn-from-guest (capability-gated)
  console.log(await calc.add(2, 3));          // → 5  (spawn + send + receive, hidden)
  for await (const n of calc.countTo(3)) console.log(n); // 1, 2, 3
}
```

```rust [Rust]
#[rusm_rs::main]
fn run() {
    let calc = calc::Client::spawn("calc").unwrap();
    assert_eq!(calc.add(2, 3).unwrap(), 5);
    for n in calc.count_to(3).unwrap() { println!("{n}"); }
}
```

```go [Go]
func run() {
	calc, _ := rusm.Spawn("calc")
	sum, _ := rusm.Call[int](calc, "add", 2, 3) // → 5
	_ = sum
}
```

:::

## 3. Reach a *running* instance — `connect`

`spawn` always starts a **fresh** instance. To talk to one that is **already running** — a
[stateful service](/build-an-app/build-a-stateful-service) declared `resident = true`, which the
node boot-spawns, supervises, and registers under its name — use `connect` (Rust/TS) or call its
pid directly (Go). You get the **same typed client**, but it **attaches** to the existing instance
instead of starting a new one, so you don't own its lifecycle (no `.stop()` — you didn't start
it). `connect` takes a registered **name** (looked up for you) or a **pid**.

::: code-group

```ts [TypeScript]
import { connect } from "rusm-ts";
import type { Calc } from "../calc"; // same contract as spawn — type-only

const calc = connect<Calc>("calc");   // attach by name (or pass a pid)
console.log(await calc.add(2, 3));     // → 5 — a real round-trip to the existing instance
```

```rust [Rust]
// Look the name up, then attach to that pid (Client::connect, not Client::spawn).
let calc = calc::Client::connect(rusm_rs::whereis("calc").unwrap());
assert_eq!(calc.add(2, 3).unwrap(), 5);
```

```go [Go]
// Go has no separate connect — look up the pid and Call it directly (Call takes any pid).
pid, _ := rusm.Whereis("calc")
sum, _ := rusm.Call[int](pid, "add", 2, 3) // → 5
```

:::

::: tip `connect` resolves a name — it does not verify a live service
`connect(name)` throws only when the **name isn't registered** (the `whereis` is empty). It does
**not** check that the target is alive or actually runs a dispatch loop — so if you `connect` to a
pid that has died, or to a process that isn't a service (a worker never replies), the **call
hangs**. This is why you `connect` to a **`resident` by name**: a resident is *supervised*
(restarted on crash) and *re-registers its name*, so the name always resolves to a live instance.
Don't hold a client across a target's crash — re-`connect` (re-resolve the name) instead. When
liveness can't be guaranteed, add a deadline with `withTimeout` / `call_timeout` / `CallTimeout`
(see [Call with a deadline](#call-with-a-deadline) below).
:::

## `spawn` or `connect`?

These are **two separate questions**, and the first one gates the second:

**1. Is the callee a service or a worker?** This decides whether it's *callable* at all.
- A **service** runs a dispatch loop and replies — so it's the **only** valid target of a typed
  call, whether you reach it with `spawn<T>` or `connect<T>`.
- A **worker** is one-shot: it reads its input, does the job, and exits. You `spawn` it and `send`
  it work (see [Run one-off work](/build-an-app/run-one-off-work)) — it never dispatches, so it is
  **never** a `connect`/typed-call target. A typed call to a worker would hang, waiting for a reply
  it never sends.

**2. For a service: a fresh instance, or an existing one?** This is where `spawn` vs `connect`
actually differ — both give you the same typed client, against a service:
- **`spawn<T>`** starts a **new** instance you **own** — `.stop()` it when done. Two
  `spawn("calc")` calls make **two** isolated processes. (Note `spawn` is also how you start a
  *worker* — but then you message it directly, you don't hold a typed call-client to it.)
- **`connect<T>`** attaches to one **already running** — typically the single `resident` the node
  boot-spawns and supervises — and hands you a client you **don't own**. Many components can
  `connect` to the *same* instance and share its supervised state.

So the verb isn't worker-vs-service; it's fresh-vs-existing **within services**. (And `connect`
only makes sense against a service for exactly that reason — point 1.)

| You want | Callee | Use | Instance | You own it? |
| --- | --- | --- | --- | --- |
| a one-shot job, then gone | worker | `spawn` + `send` | fresh | yes — exits on return |
| your own private instance | service | `spawn<T>` | fresh | yes — `.stop()` it |
| the shared, long-lived instance | service (`resident`) | `connect<T>` | existing | no — you didn't start it |

## 4. Call with a deadline {#call-with-a-deadline}

A plain `call` blocks until the reply arrives — forever if the target is dead or not a service.
Use a **deadline** any time the callee's liveness isn't guaranteed:

::: code-group

```ts [TypeScript]
import { spawn, callTimeout } from "rusm-ts";
import type { Calc } from "../calc";

// Option A — timeout on the proxy; every subsequent call uses it.
const calc = spawn<Calc>("calc").withTimeout(500); // 500 ms deadline per call
const result = await calc.add(2, 3);               // throws Error("timeout") if overdue

// Option B — ad-hoc one-off call with a deadline.
const sum = await callTimeout(pid, "add", 500, 2, 3); // (pid, op, timeoutMs, ...args)
```

```rust [Rust]
use rusm_rs::wire::call_timeout;

// Typed client: no built-in per-method timeout yet — use the wire function directly.
let result: Result<i64, String> =
    call_timeout(calc_pid, "add", &[2_i64, 3_i64], 500); // 500 ms; Err("timeout") on expiry
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

// CallTimeout[R](to, op, timeoutMs, ...args) — drops into Err("timeout") when the deadline fires.
sum, err := rusm.CallTimeout[int](calcPid, "add", 500, 2, 3) // 500 ms
if err != nil { /* err.Error() == "timeout" on expiry */ }
```

:::

**How it works.** Every SDK tracks a `Instant` / `time.Until` / `Date.now()` deadline that spans
the *entire* call — including any non-matching mail that must be set aside and restored to the
inbox first. When the remaining time reaches zero before the matching reply arrives, the call
returns `Err("timeout")` / throws `Error("timeout")` and restores all set-aside messages to the
front of the inbox, so the process's own receive sees them in order.

## What you need to know

- **Capability-gated.** Spawning is the `allow-spawn` capability — grant it on the caller's
  profile (see [Grant capabilities](/build-an-app/grant-capabilities)). A node-registered component
  runs under **its own** declared profile, whoever spawns it, so its secrets stay scoped to
  it.
- **Per-call vs long-lived.** `spawn` gives you a **fresh** instance each call; `connect`
  attaches to one **already running** (see [`spawn` or `connect`?](#spawn-or-connect) above). For a
  *single long-lived* instance many callers share, make it a
  [stateful service](/build-an-app/build-a-stateful-service) (`resident = true`) and `connect` to it
  by name. For a fire-and-forget one-shot, see [Run one-off work](/build-an-app/run-one-off-work).
- **It's just messages.** The typed client is sugar over `send`/`receive`
  ([process management](/build-an-app/coordinate-and-supervise)); a Rust client and a TS
  service interoperate over the same JSON wire.
- **Deadlines.** Use `withTimeout` / `call_timeout` / `CallTimeout` any time the callee's
  liveness isn't guaranteed — a `spawn`ed service whose instance might crash, or a `connect`
  target during a rolling restart. The timeout fires as `Err("timeout")` / `Error("timeout")`,
  and all set-aside mail is restored to the inbox front.
