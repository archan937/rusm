# Phase 8 — guest ergonomics

**Goal:** make writing a RUSM guest *pleasant* — in **TypeScript** or **Rust** —
so a component is just exported functions, and calling another component reads like
a local function call. The actor ABI from Phase 7 is powerful but raw; Phase 8 is
the ergonomic layer on top, in both languages, over one shared wire.

## Why this matters

Phase 7 proved a component can be a supervised, addressable process. But a guest
still hand-rolled bindings and hand-parsed bytes. Phase 8 delivers the developer
experience: **services** (exported functions), a **concealed typed client**
(`spawn` + send + receive, hidden behind `await svc.method(...)`), **streaming**
and **callbacks**, an in-guest **`Supervisor`**, and `rusm dev` watch + reload —
the same story for Rust and TS, interoperable because they share one JSON wire.

## What we built (TDD throughout)

1. **rusm-ts** — a TypeScript guest is plain TS, Bun-bundled (`rusm build` →
   `bun build --format=cjs`) and run on the shared rquickjs **js-runner**. A
   **service** just `export`s functions (RUSM runs the receive→dispatch→reply
   loop); a **worker** is `export default async function`. The `Process` actor API
   is **async** (`await Process.receive()` — the host call suspends the instance's
   fiber, so "blocking" stays cheap). Web APIs (`URL`, `TextEncoder`, `Headers`,
   `ReadableStream`, `console`, `crypto`) are polyfilled, and a capability-gated
   streaming **`fetch`** rides the host's `wasi:http` client (refused for a sandboxed
   guest). (Both shipped with Phase 11.)
2. **The concealed typed client** — `spawn<Svc>("svc")` returns a proxy
   whose `await svc.method(args)` is a real cross-process call. Generator handlers
   **stream** (`for await (const x of svc.gen(...))`); function arguments are
   **callbacks** that stay in the caller (their invocations travel back as
   messages); `svc.cast.method(...)` is fire-and-forget. Wire: JSON
   `{op,args,from,ref}` → `{ref,ok|err}`, with `{op:"__cb"}` callbacks.
3. **rusm-rs** — the Rust twin: ergonomic `Pid`/`send`/`receive` (serde JSON, the
   *same* wire as TS)/`spawn`/registry/`Stream` over the actor world, via the
   wit-bindgen **library/binary split** (rusm-rs owns an imports-only world; a
   guest maps the actor interface to it and `export!`s its own `run`, so the
   interface is imported exactly once). A `#[rusm_rs::service]` macro over a `mod`
   of free functions (mirroring TS's `export function`s — no `impl`, no `self`)
   generates a `serve()` dispatch loop **and** a typed `Client` with
   call/cast/streaming/callbacks. A Rust client and a TS service interoperate.
4. **Spawn-from-guest + monitor** (actor ABI) — `spawn` instantiates a registered
   component by name → a new pid; `monitor` makes a dead process arrive as a
   `__down` message (`receive` translates the runtime `Down` — no watcher process,
   no polling). Both are **capability-gated**; the `allow-spawn` capability gates who may
   spawn, and a node-registered component runs under its own manifest-declared profile
   (a guest can't fabricate capabilities the operator never granted).
5. **In-guest `Supervisor`** — in both rusm-rs and rusm-ts: spawn + monitor named
   children and restart per strategy — `one_for_one` / `one_for_all` /
   `rest_for_one`, with `max_restarts` (overload protection). The OTP supervision
   tree, written from inside a guest.
6. **The `rusm-ts` npm package** — the TS guest API ships as an importable package
   (`import { Process, spawn, supervise } from "rusm-ts"`), added with `bun add rusm-ts`;
   `rusm build` runs `bun install` for you. (Root-caused a subtle hang doing
   this: the runner now wraps each bundle in a **CommonJS scope**, so a bundle's
   top-level `var` can never clobber the runtime globals — correct CJS isolation.)
7. **Custom capability profiles** — `rusm.toml` accepts `[capabilities.<name>]`
   profiles, Cargo-style: each `inherits` a built-in base and overrides specific
   grants (`allow-network` / `allow-spawn` / `allow-process-control` / `allow-stdio` / `max-memory-mb` /
   `env` / `preopen`). A component selects one by name.
8. **`rusm dev` watch + reload** — `rusm dev` builds, runs, and **watches**
   `./components`; on a source edit it rebuilds and reloads the components. A
   dependency-free mtime poll (skips `target/` and `node_modules/`).

## Concepts introduced

- [Components & the actor world](../concepts/components-and-the-actor-world.md) —
  the actor ABI the guest crates wrap; composition is message passing.
- [Permissions & sandboxing](../concepts/permissions-and-sandboxing.md) — `spawn`,
  `monitor`, and custom `[capabilities.<name>]` profiles.
- The full guest story (TS + Rust, service / client / supervisor) — see
  [Getting started](../getting-started.md), the [`rusm-ts`](https://github.com/archan937/rusm)
  package, and the `rusm-rs` crate.

## Play with it

```sh
# The TypeScript todo board — a `store` service + a `reporter` worker that drives it
# through the typed client (call + cast + streaming + callback). Build with Bun, run on RUSM:
cd examples/typescript
rusm build      # bun install (if needed) + bundle each components/<name>/index.ts
rusm serve      # boots the resident reporter → logs the call / seed-callback / stream / cast
rusm dev        # same, then watch & reload on edit
```

::: code-group

```rust [Rust]
// A Rust service — functions become a dispatch loop + a typed Client:
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n }     // streaming
    pub fn work(progress: rusm_rs::Callback<i64>) -> String { /* … */ } // callback
}
// caller:  let calc = calc::Client::spawn("calc")?;  calc.add(2, 3)?;
```

```ts [TypeScript]
// A TS service — exported functions become a dispatch loop; the contract is derived:
export function add(a: number, b: number): number { return a + b; }
export function* countTo(n: number) { for (let i = 1; i <= n; i++) yield i; } // streaming
export function work(progress: (pct: number) => void): string {              // callback
  for (const pct of [25, 50, 100]) progress(pct);
  return "done";
}
export type Calc = typeof import(".");
// caller:  const calc = spawn<Calc>("calc");  await calc.add(2, 3);
```

```go [Go]
// A Go service — register typed handlers; a generic Call[R] client speaks the wire:
func run() {
	svc := rusm.NewService()
	svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
	svc.HandleStream("countTo", func(req rusm.Request, out rusm.Sink) error { // streaming
		n, _ := rusm.Arg[int](req, 0)
		for i := 1; i <= n; i++ {
			out.Send(i)
		}
		return nil
	})
	svc.Handle("work", func(req rusm.Request) (any, error) { // callback
		progress := rusm.CallbackArg(req, 0)
		for _, pct := range []int{25, 50, 100} {
			progress.Call(pct)
		}
		return "done", nil
	})
	svc.Serve()
}
// caller:  sum, _ := rusm.Call[int](calc, "add", 2, 3)
```

:::

## Verification

`cargo test` green — host-level spawn gating + per-component declared profiles, the JS service
dispatch (sync + async handlers), a TS commander calling a service via the typed
client (call + streaming + callback), the Rust `#[service]` macro driven end to end
(call + streaming + callback), and a `Supervisor` (Rust **and** TS) restarting a
killed child. The Bun-built `typescript` todo-board example runs end to end; the component-storm
spawn path holds **~440k spawns/sec** (no regression from spawn-from-guest). The
Wasm-free invariant still holds (no `wasmtime` under `rusm-otp`).

**Reclassified to Phase 11:** a native p3-typed `stream<u8>` WIT signature — a
standards-surface refinement; the byte streams already work over a handle ABI.

## Next

[Phase 9](../02-roadmap.md): **distributed clusters + live attach** — QUIC + TLS,
remote spawn, and a global registry, so processes spawn and message across nodes.
