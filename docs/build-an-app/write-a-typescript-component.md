# Write a TypeScript component

You write TypeScript. RUSM bundles it with Bun and runs it as a **sandboxed, supervised
process** — isolated memory, capability-gated I/O, crash-recovered by the supervisor.
No jco, no boilerplate, no raw WASM tooling. Write logic; RUSM handles the rest.

## Scaffold & run in 30 seconds

```sh
rusm new myapp       # scaffold a TypeScript HTTP component
cd myapp
rusm build           # bun build → wasm/api.js
rusm serve           # live on http://127.0.0.1:8080
```

Want WebSocket or SSE instead?

```sh
rusm new myapp --protocol ws    # WebSocket component
rusm new myapp --protocol sse   # Server-Sent Events component
```

A component is a folder under `components/` with a single `index.ts`:

```
my-app/
├── rusm.toml
├── components/
│   └── api/
│       └── index.ts
└── wasm/                   # rusm build writes api.js here
```

## Two shapes

### Service — export functions

Export named functions. RUSM generates the receive → dispatch → reply loop around them
automatically. The caller gets a **typed proxy** — ordinary `await` calls that are
actually cross-process messages:

```ts
// components/calc/index.ts
export function add(a: number, b: number): number { return a + b; }
export async function greet({ name }: { name: string }) { return `hi ${name}`; }

// Publish the contract — derived from the exports above, so it never drifts.
export type Calc = typeof import(".");
```

### Worker — export default

Export a `default` async function. RUSM runs it once; it does its job and exits.
Use the **typed client** to reach a service — `spawn<T>` returns a proxy where every
method is a real cross-process call hidden behind `await`:

```ts
// components/commander/index.ts
import { spawn } from "rusm-ts";
import type { Calc } from "../calc";   // type-only import — never bundled in

export default async function () {
  const calc = spawn<Calc>("calc");

  console.log("2 + 3 =", await calc.add(2, 3));             // typed call
  for await (const n of calc.countTo(3)) console.log(n);    // streaming
  await calc.work((pct) => console.log(`progress ${pct}`)); // callback
}
```

`calc` stays a separate component — the `import type` is erased at build time, so nothing
from `calc` is bundled into `commander`. They communicate over messages, not imports.

## Declare in `rusm.toml`

Every component needs an entry. The spawner needs the `allow-spawn` capability:

```toml
[components.calc]
capability = "sandboxed"

[components.commander]
capability = "trusted"   # inherits allow-spawn
```

## Add the SDK

```json
{ "dependencies": { "rusm-ts": "^0.5.0" } }
```

`rusm build` runs `bun install` automatically before bundling.

## Build & run

```sh
rusm build   # bun build → wasm/*.js for every component
rusm run     # spawn them per rusm.toml
rusm dev     # build + run, then watch ./components and hot-reload on every save
```

## The `Process` API

The full actor toolkit — all typed by `rusm-ts`:

| | |
|---|---|
| `Process.self()` | this process's pid |
| `Process.send(pid, msg)` | send a message |
| `Process.receive()` / `receiveText()` | wait for a message (suspends the fiber — cheap) |
| `Process.spawn(name)` / `spawn<T>(name)` | spawn a component; typed client variant |
| `Process.register(name)` / `whereis(name)` | named registry |
| `Process.registerTag(tag)` / `whereisTag(tag)` | process-group tags |
| `Process.kill(pid)` | exit another process |
| `Process.openStream()` / `acceptStream()` | byte streams |

`receive` and `receiveText` are `async` — they suspend the fiber while waiting, freeing
the scheduler for other work. No threads, no event loop fights.

**`fetch` works** — capability-gated. A `network-client` guest gets full HTTPS with streaming
bodies and `AbortSignal`; a `sandboxed` guest's `fetch` rejects with a clear error.
`crypto.getRandomValues` and `randomUUID` are available to every guest.

Add `"lib": ["ES2022", "DOM"]` to your `tsconfig.json` for the Web API types.

::: tip Logging is zero-config
`console.log/info/warn/error/debug` routes to the node's unified log stream — stamped with
the time, `component#pid`, and a severity colour. No setup, no `allow-stdio` grant, no
logger object. Gated by `[log] level` in `rusm.toml`.
:::

## How TypeScript runs in RUSM

RUSM ships a single **~920 KB js-runner** — [QuickJS](https://bellard.org/quickjs/) compiled
to `wasm32-wasip2` via [rquickjs](https://github.com/DelSkayn/rquickjs). This shapes
everything about how TS components perform:

**Wizer pre-initialization.** At build time, [wizer](https://github.com/bytecodealliance/wizer)
boots the QuickJS engine and the full JS bridge — all `Process.*`, `fetch`, `crypto`, and `kv`
primitives — and snapshots the result into the binary. Every spawned instance
**copy-on-write starts from that warm snapshot**: the engine never boots from scratch at
runtime; each spawn only evaluates your Bun-bundled `.js`. This gives roughly **8× better
cold per-request throughput** vs a non-pre-initialized runner.

**One engine, every component.** All your TypeScript components share the same js-runner
binary. You ship the engine once. Each spawned instance starts from the wizer snapshot and
the OS uses **copy-on-write (CoW)** to share it: every instance reads from the same physical
memory pages until it writes to one, at which point only that page is copied for that
instance. The 920 KB engine image is never duplicated in full — you pay only for the pages
each instance actually diverges from the snapshot.

### RUSM TS components vs ComponentizeJS

The closest comparison for running TypeScript as a sandboxed Wasm process is
[ComponentizeJS](https://github.com/bytecodealliance/ComponentizeJS) — the Bytecode Alliance
tool that compiles JS/TS to a Wasm component by embedding
[StarlingMonkey](https://github.com/bytecodealliance/StarlingMonkey) (a SpiderMonkey variant).
[JCO](https://github.com/bytecodealliance/jco) then transpiles those components to run in
Node.js or a browser.

Both approaches run JavaScript inside Wasm. The differences are in engine sharing, actor
model, and operational model:

| | **RUSM + rquickjs** | **ComponentizeJS + JCO** |
|---|---|---|
| **JS engine** | QuickJS (~920 KB), shared across all TS components on a node | StarlingMonkey (~8 MB), embedded separately in **each** component |
| **Your component artifact** | `.js` bundle of your code (2–50 KB) | `.wasm` with engine included (~8 MB per component) |
| **Wizer pre-init** | ✓ engine + bridge snapshotted once at build time | ✓ per-component snapshot |
| **Engine sharing** | ✓ CoW-shared; one copy in memory regardless of instance count | ✗ each component carries its own ~8 MB copy |
| **Default-deny capabilities** | ✓ per-process, host-enforced | ✗ WASI shims in Node.js host; no default-deny model |
| **Memory cap per instance** | ✓ `StoreLimiter` | ✗ |
| **Epoch preemption** | ✓ a spinning guest can't starve others | ✗ |
| **Actor model** | ✓ supervised, addressable, killable, mailbox | ✗ |

The engine-sharing gap is the headline. A node running ten RUSM TypeScript components holds
one ~920 KB QuickJS image in memory, CoW-shared across all instances. Ten ComponentizeJS
components each carry their own ~8 MB StarlingMonkey — 80 MB before a single line of your
code. RUSM's approach keeps the total engine footprint fixed at ~920 KB regardless of how
many TS components you deploy.

## Go deeper

- [Call another component](/build-an-app/call-another-component) — typed clients, `connect` to a resident, call with a deadline
- [Serve HTTP / WS / SSE](/build-an-app/serve-http) — turn a component into a high-throughput server
- [Coordinate & supervise](/build-an-app/coordinate-and-supervise) — links, monitors, supervisors
- [Runnable todo-board](https://github.com/archan937/rusm/tree/main/examples/todo-board/typescript) — service + worker + streaming + callback, end to end
