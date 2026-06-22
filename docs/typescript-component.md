# Write a TypeScript component

TypeScript guests are **first-class, sandboxed RUSM processes** — the
`genius-wasmcloud` model, **no jco**. RUSM ships one **js-runner** component: it
embeds [rquickjs](https://github.com/DelSkayn/rquickjs) (QuickJS, compiled to
`wasm32-wasip2`, ~920 KB) and runs your JS, exposing a `Process` global bridged to
the actor world. You write TS; **Bun** bundles it to one `.js`; the runner
executes it inside the same sandbox (capabilities, memory cap, epoch preemption)
as a Rust component. A TS component is just a folder with an `index.ts`:

A TS component comes in two shapes. A **service** just exports functions — RUSM
runs the receive→dispatch→reply loop around them:

```ts
// components/calc/index.ts
export function add(a: number, b: number): number { return a + b; }
export async function greet({ name }: { name: string }) { return `hi ${name}`; }

// Publish the contract — derived from the functions above, so it never drifts.
// `import(".")` is this component's own directory (the same way a caller writes
// `from "../calc"`); it resolves to this index, so the type is "all my exports".
export type Calc = typeof import(".");
```

A **worker** exports a `default` (async) function — RUSM runs it once. It reaches a
service through the **typed client**: `spawn<Calc>("calc")` returns a proxy whose
calls are real cross-process messages, hidden behind `await`. The caller imports only
the service's **contract** — a `type`, erased at build, so `calc` is never bundled in;
it stays a separate component reached over messages:

```ts
// components/commander/index.ts
import { spawn } from "rusm-ts";
import type { Calc } from "../calc";          // type-only — the contract, not the code

export default async function () {
  const calc = spawn<Calc>("calc");            // spawn-from-guest, capability-gated
  console.log("2 + 3 =", await calc.add(2, 3)); // call: spawn + send + receive, hidden

  // A generator handler streams: `for await` its chunks.
  for await (const n of calc.countTo(3)) console.log(n);

  // A function argument is a callback — it stays here; the service's calls come
  // back as messages routed to it.
  await calc.work((pct) => console.log(`progress ${pct}`));
}
```

Declare both in `rusm.toml`, with capability profiles (the commander needs the
`allow-spawn` capability — here a custom profile inheriting `trusted`):

```toml
[capabilities.orchestrator]
inherits = "trusted"

[components.calc]
capability = "sandboxed"

[components.commander]
capability = "orchestrator"
```

The `Process` API and `spawn` come from the **`rusm-ts` package** — add it to your
app's `package.json`:

```json
{ "dependencies": { "rusm-ts": "^0.3.0" } }
```

`rusm build` runs `bun install` (if needed), then detects each `index.ts` and runs
`bun build --format=cjs` → `wasm/<name>.js` (a Rust component builds to
`wasm/<name>.wasm` instead — same manifest, same loader). `rusm run` loads `.js`
artifacts on the shared js-runner and prints:

```
2 + 3 = 5
hi RUSM
```

`receive`/`receiveText` and `Stream.read` are **async** (`await`) — the host call
still suspends the whole instance's fiber (freeing the worker), so it's cheap and
composes with Promises. The full `Process` API (`self`/`list`/`spawn`/`send`/
`receive`/`receiveText`/`register`/`whereis`/`isAlive`/`kill`/`setLabel`/
`registerTag`/`killTag`/`whereisTag` ([process groups](./concepts/process-management#process-groups-tags))/
`openStream`/`acceptStream`), the `spawn<T>()` typed client (call / `for await`
stream / callback args / `.cast` / `.stop()`), binary (`Uint8Array`) messages, and
[byte streams](./concepts/byte-streams) are all typed by the **`rusm-ts` package**.
The Web APIs the runner polyfills (`URL`, `TextEncoder`, `Headers`,
`ReadableStream`, `console`) are typed by the standard `DOM` lib — add it to your
`tsconfig.json` (`"lib": ["ES2022", "DOM"]`). See the runnable `typescript` todo-board
example (its `store` service + `reporter` worker, with streaming + a callback) and
`host_ts_component`.

> **Outbound `fetch` works — capability-gated.** A guest granted network (the
> `network-client` profile) can `fetch` over the host's `wasi:http` client — HTTPS,
> streaming bodies, `AbortSignal`. A *sandboxed* guest's `fetch` is refused at the host
> (default-deny) and rejects with a clear error. `crypto` (`getRandomValues`/
> `randomUUID`) is available to every guest.

**The Rust twin — `rusm-rs`.** A Rust guest gets the same story without raw
wit-bindgen: `Pid`/`send`/`receive` (serde)/`spawn`/registry/`Stream`, plus a
`#[rusm_rs::service]` macro over a module of free functions (mirroring TS's
`export function`s) that generates a `serve()` dispatch loop and a typed `Client`:

```rust
#[rusm_rs::service]
pub mod calc {
    pub fn add(a: i64, b: i64) -> i64 { a + b }
    pub fn count_to(n: i64) -> impl Iterator<Item = i64> { 1..=n }   // streaming
    pub fn work(progress: rusm_rs::Callback<i64>) -> String {        // callback
        for pct in [25, 50, 100] { progress.call(pct); } "done".into()
    }
}
// caller:  let calc = calc::Client::spawn("calc")?;  calc.add(2, 3)?;
```

Same JSON wire as rusm-ts, so a Rust client and a TS service interoperate. See the
`rusm-rs` crate README and the `rs-service` fixture.

**Logging — zero setup, all three languages.** A guest just uses the native idiom; the
platform does the rest. The host stamps each line with the time, the calling
`component#pid`, and a severity colour, and gates it by the node `[log] level` — so a
guest never wires a name, pid, or logger object:

```ts
console.log(`generating ${req.collection}/${req.subjectId}`); // TS: console.{log,info,warn,error,debug}
console.error("meta-json not found");
```

```rust
log::info!("generating {}/{}", req.collection, req.subject_id); // Rust: the `log` crate
log::error!("meta-json not found");
```

No `allow-stdio` grant — logging is a platform primitive, not stdout. The `console`
methods are also typed by the standard `DOM` lib, and the `log` crate's sink is installed
for you by `#[rusm_rs::main]` / `#[handlers]`. Both feed the same stream as the runtime's
own lifecycle lines; see the [`[log]` reference](./reference-configuration).
