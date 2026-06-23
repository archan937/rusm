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

::: warning `connect` resolves a name — it does not verify a live service
`connect(name)` throws only when the **name isn't registered** (the `whereis` is empty). It does
**not** check that the target is alive or actually runs a dispatch loop — and a typed `call`
currently has **no timeout** (it blocks until the reply arrives). So if you `connect` to a pid that
has died, or to a process that isn't a service (a worker never replies), the **call hangs**, it
doesn't error. This is why you `connect` to a **`resident` by name**: a resident is *supervised*
(restarted on crash) and *re-registers its name*, so the name always resolves to a live instance.
Don't hold a client across a target's crash — re-`connect` (re-resolve the name) instead. (Same
caveat applies to a `spawn`ed client whose instance dies mid-call.)
:::

## `spawn` or `connect`?

A common mix-up: this is **not** service-vs-worker. The axis is **a fresh instance vs an existing
one**:

- **`spawn`** starts a **new** instance and hands you a client you **own** — use it, `.stop()` it,
  or (for a worker) let it exit when its one job is done. Two `spawn("calc")` calls make **two**
  isolated `calc` processes.
- **`connect`** attaches to an instance **already running** (typically the one shared `resident`)
  and hands you a client you **don't own** — many components can `connect` to the *same* instance
  and talk to its shared, supervised state.

So the component's *kind* doesn't pick the verb — what you want from it does:

- A **worker** is always `spawn`ed: it's one-shot and ephemeral, so there's no shared instance to
  attach to.
- A **service** can be **either** — `spawn` a fresh private instance you own and stop, **or**
  `connect` to the one long-lived `resident` the node supervises (the usual case for a registry,
  counter, cache, or broker that the whole app shares).

| You want | Use | Instance | You own its lifecycle? |
| --- | --- | --- | --- |
| a one-shot job, then gone | `spawn` | fresh | yes — it exits on return |
| your own private service instance | `spawn` | fresh | yes — `.stop()` it |
| to reach the shared, long-lived service | `connect` | existing (`resident`) | no — you didn't start it |

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
