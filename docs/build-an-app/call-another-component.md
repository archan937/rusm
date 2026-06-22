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

## What you need to know

- **Capability-gated.** Spawning is the `allow-spawn` capability — grant it on the caller's
  profile (see [Grant capabilities](/build-an-app/grant-capabilities)). A node-registered component
  runs under **its own** declared profile, whoever spawns it, so its secrets stay scoped to
  it.
- **Per-call vs long-lived.** `spawn` gives you a **fresh** instance each call. For a *single
  long-lived* instance many callers share, make it a
  [stateful service](/build-an-app/build-a-stateful-service) (`resident = true`) that registers a
  name — then reach it by pid (Rust/Go) or share its state through `kv`. For a fire-and-forget
  one-shot, see [Run one-off work](/build-an-app/run-one-off-work).
- **It's just messages.** The typed client is sugar over `send`/`receive`
  ([process management](/build-an-app/coordinate-and-supervise)); a Rust client and a TS
  service interoperate over the same JSON wire.
