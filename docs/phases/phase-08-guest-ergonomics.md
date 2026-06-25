# Phase 8 — Guest ergonomics

Phase 7 proved a component can be a supervised process. Phase 8 makes writing one feel natural — in TypeScript, Rust, or Go, over one shared wire, with a typed client that turns cross-process calls into ordinary function calls.

## Why this phase

The raw actor ABI from Phase 7 works. But a guest hand-rolling `send(pid, json_encode({op: "add", args: [2, 3]}))` and parsing `recv()` bytes for a reply is friction that would keep RUSM a systems project instead of a platform. Phase 8 delivers the developer experience: export a function in TypeScript, call it from Rust or Go with full type safety, get a streaming response back — all over messages, none of the plumbing visible.

Three languages, one wire. A TypeScript service and a Go caller interoperate out of the box. The wire is JSON; the SDKs agree on the shape.

## What shipped

1. **rusm-ts** — a TS guest is plain TypeScript, Bun-bundled (`rusm build` → `bun build --format=cjs`), run on the shared **rquickjs js-runner**. A service just `export`s functions; a worker is `export default async function`. The `Process` API is `async` — `await Process.receive()` suspends the instance's fiber cheaply, no threads. Web APIs (`URL`, `Headers`, `ReadableStream`, `console`, `crypto`, `fetch`) are available, with `fetch` capability-gated.
2. **rquickjs + wizer pre-initialization** — RUSM embeds [QuickJS](https://bellard.org/quickjs/) compiled to `wasm32-wasip2` via [rquickjs](https://github.com/DelSkayn/rquickjs) as the shared js-runner (~920 KB total). At build time, [wizer](https://github.com/bytecodealliance/wizer) boots the engine and the full JS bridge — all `Process.*`, `fetch`, `crypto`, `kv` primitives — and snapshots the result. Every spawned instance **copy-on-write starts from that warm snapshot**: the engine never boots from scratch; each spawn only evaluates the user's Bun-bundled `.js`. This gives roughly **8× better cold per-request throughput** vs a non-pre-initialized runner, and all TS components share one ~920 KB binary.
3. **The typed client** — `spawn<Svc>("svc")` returns a proxy whose `await svc.method(args)` is a real cross-process call. Generator handlers stream (`for await (const x of svc.gen(...))`); function arguments are callbacks that stay in the caller; `svc.cast.method(...)` is fire-and-forget.
4. **rusm-rs** — the Rust twin: `Pid`/`send`/`receive` (serde JSON, same wire as TS)/`spawn`/registry/`Stream` over the actor world. A `#[rusm_rs::service]` macro over a `mod` of free functions generates a `serve()` dispatch loop *and* a typed `Client` with call/cast/streaming/callbacks. A Rust client and a TS service interoperate directly.
5. **rusm-go** — the Go peer: TinyGo → `wasm32-wasip2`, idiomatic `Pid`/`Send`/`Receive`/`Spawn`, a `NewService()` of typed handlers, and a generic `Call[R]` client — all on the same JSON wire as TS and Rust. `rusm new --lang go`, `rusm build` drives TinyGo.
6. **Spawn-from-guest + monitor** — `spawn` instantiates a registered component by name from inside a guest; `monitor` makes a dead process arrive as a `__down` message. Both capability-gated: the manifest declares what a component can do; a guest can't fabricate grants.
7. **In-guest `Supervisor`** — in both rusm-rs and rusm-ts: spawn + monitor named children and restart per strategy (`one_for_one` / `one_for_all` / `rest_for_one`) with `max_restarts`. The OTP supervision tree, written from inside a component.
8. **`rusm dev` watch + reload** — builds, runs, and **watches** `./components`. On a source edit it rebuilds and reloads automatically. A dependency-free mtime poll (skips `target/` and `node_modules/`).
9. **Custom capability profiles** — `rusm.toml [capabilities.<name>]` profiles, each inheriting a built-in base with specific overrides. A component selects one by name.

## Design highlights

- **Three languages, one wire.** The JSON `{op, args, from, ref}` → `{ref, ok|err}` wire is the contract. TS exports `function add(a, b)`, Rust calls `calc.add(2, 3)`, Go calls `rusm.Call[int](calc, "add", 2, 3)`. No special interop layer needed — the wire IS the interop.
- **rquickjs over JCO.** JCO transpiles Wasm *to* JavaScript to run in Node/Deno/Bun — the opposite direction. It gives no sandbox, no capability gating, and a full V8 instance per deployment. rquickjs runs JS *inside* the Wasm sandbox, with the full RUSM isolation model, at ~920 KB shared across all components.
- **Wizer snapshot = near-zero per-spawn engine cost.** The QuickJS engine and full bridge are booted once at build time. Runtime spawn cost is CoW page mapping + evaluating the user's bundle. No engine initialization on the critical path.
- **`import type` erasure.** A TS caller does `import type { Calc } from "../calc"` — erased at build time, so nothing from `calc` is bundled into the caller. Components communicate over messages, not imports. The type is just a type.

## What this unlocks

Writing a RUSM component in any of the three languages is now as simple as exporting functions. The typed client, streaming, and callbacks make cross-component composition feel local. `rusm dev` closes the inner loop to seconds.

The todo-board example shows the complete picture: a `store` service, a `reporter` one-shot, and a streaming `feed` — in TypeScript, Rust, and Go — all interoperating over the same wire.

## Try it

```sh
# TypeScript todo-board — service + one-shot + streaming + callback:
cd examples/todo-board/typescript
rusm build && rusm serve

# Rust todo-board:
cd examples/todo-board/rust
rusm build && rusm serve

# Go todo-board:
cd examples/todo-board/go
rusm build && rusm serve
```

::: code-group

```ts [TypeScript service]
export function add(a: number, b: number): number { return a + b; }
export function* countTo(n: number) { for (let i = 1; i <= n; i++) yield i; }
export type Calc = typeof import(".");
// caller: const calc = spawn<Calc>("calc"); await calc.add(2, 3)
```

```rust [Rust service]
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n }
}
// caller: let calc = calc::Client::spawn("calc")?; calc.add(2, 3)?
```

```go [Go service]
svc := rusm.NewService()
svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
svc.Serve()
// caller: sum, _ := rusm.Call[int](calc, "add", 2, 3)
```

:::

## Status

Phase complete. All three SDKs (TS, Rust, Go) ship and interoperate. Component-storm holds ~440k spawns/sec — no regression. `rusm dev` watch+reload works across all three languages.

---

*Next: [Phase 9](./phase-09-distributed-clusters.md) — distributed clusters: same send, same registry, now spanning machines over QUIC + mTLS at ~550k cross-node msgs/sec.*
