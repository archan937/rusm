# Getting started {#overview}

RUSM runs WebAssembly components as isolated, supervised, Erlang-style processes and
serves them over HTTP/WS/SSE. The fastest way in is to **scaffold an app and serve it** —
then read on for the actor core, embedding, and writing your own components. Every command
here is real and current; anything not yet built is marked **Roadmap**.

## Quick start {#quick-start}

From nothing to a live server in four commands:

```sh
rusm new hello && cd hello   # scaffold a TS HTTP component + rusm.toml
rusm build                   # components/ → wasm/
rusm serve                   # → http://127.0.0.1:8080
curl http://127.0.0.1:8080/  # "Hello from RUSM 👋"
```

> Just need the CLI? [**Install `rusm`**](#install) — one `cargo install`; the only
> prerequisites are Rust and (for TypeScript components) Bun.

### Scaffold a real app — the TODO board

Want a real app instead of hello world? Scaffold the full **TODO board** — HTTP CRUD, a
live SSE feed, WebSocket Chat, and a resident `store` service driven by a worker — in
TypeScript, Rust, or Go:

```sh
rusm new board --template todo-board   # add --lang rust or --lang go (default: ts)
cd board && rusm build && rusm serve   # → open http://127.0.0.1:8080
```

`rusm new --rust` (or `--lang go`) scaffolds a single Rust or Go component; `--protocol
ws|sse` a WebSocket or SSE handler.

The scaffolded `rusm.toml` is the app manifest — see the
[configuration reference](./reference-configuration.md) for every table and field
(`[[serve]]`, `[serve.routes]`, `[capabilities.<name>]`, `[components.<name>]`, env), and the
[`rusm` CLI reference](./reference-cli.md) for the full command set.

### See it live — the dashboard

The benchmark dashboard lives in the repo, so clone it to run:

```sh
git clone https://github.com/archan937/rusm && cd rusm
make dashboard      # builds + starts a node, then the dashboard — open the printed URL
```

Pick a scenario (e.g. **spawn storm**, **stream pipe**), hit **Run**, and watch real
throughput, latency, and the live observer. Everything is driven by the real runtime.

| Command | What it does |
| --- | --- |
| `make dashboard` | Build + start the benchmark node, then the dashboard (the headline demo). |
| `make node` | Start the benchmark node on `ws://127.0.0.1:4000` (release). |
| `make ui` | Start only the dashboard (expects a node already running). |
| `make run SCENARIO=… SECONDS=…` | Run a benchmark scenario in the terminal. |
| `make example EX=…` | Run a bundled example (`host_components`, …). |
| `make test` / `make cov` | All Rust + dashboard tests / coverage. |
| `make fmt` / `make fmt-check` | Format / check (Rust + dashboard). |
| `make docs` / `make docs-build` | Live-preview / build this docs site. |

Run `make` with no target for the full list.

## Install

Install the `rusm` CLI — the app model (scaffold, build, serve):

```sh
cargo install rusm-cli
```

Prerequisites:

- **Rust** 1.94+ via [`rustup`](https://rustup.rs). To build guest components, add the
  Wasm target: `rustup target add wasm32-wasip2` (and `wasm32-wasip1` for core modules).
- **Bun** 1.3+ ([bun.sh](https://bun.sh)) — to build TypeScript components; never Node.js.

Building *with* RUSM as a library (the OTP-core and embedding examples below)? Add the
crates to your own project instead:

```sh
cargo add rusm-otp           # the Wasm-free actor core
cargo add rusm-wasm          # + the Wasmtime backend, to host components
```

## Two ways to use RUSM

1. **As a library you embed** — depend on the `rusm-otp` core (and optionally the
   `rusm-wasm` backend) and drive it from your own Rust binary. Best when RUSM is a
   piece of a larger app.
2. **As an app you run** — declare components in `rusm.toml` and let the `rusm` CLI
   build, load, and supervise them. Best for a RUSM-first project.

The next sections cover both.

## 1. Without a Wasm runtime — the OTP core {#otp-core}

RUSM's heart is a **Wasm-free** Erlang/OTP actor library, `rusm-otp`. You can use
it on its own — real lightweight processes, message passing, links, monitors,
supervision, a registry, and timers — with **no WebAssembly at all**. This is the
model RUSM is built on, and it stands alone (the dependency graph guarantees it).

```rust
use rusm_otp::{ExitReason, Received, Runtime};

#[tokio::main]
async fn main() {
    let rt = Runtime::new();

    // A worker: receive one message, then exit.
    let worker = rt.spawn(|mut ctx| async move {
        if let Received::Message(bytes) = ctx.recv().await {
            println!("worker got {} bytes", bytes.len());
        }
    });

    // Supervise it: monitor delivers a `Down` with the exit reason.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let watcher = rt
        .spawn(move |mut ctx| async move {
            if let Received::Down { reason, .. } = ctx.recv().await {
                let _ = tx.send(reason);
            }
        })
        .pid();
    rt.monitor(watcher, worker.pid());

    rt.send(worker.pid(), b"hello".to_vec()); // messages are bytes (Vec<u8>)
    assert_eq!(rx.await.unwrap(), ExitReason::Normal);
}
```

You also get `spawn_link` (crash propagation), `trap_exit`, a named **registry**
(`register`/`whereis`), **timers** (`send_after`/`cancel`), graceful `shutdown`,
and **TCP** (`listen`/`connect`, one process per connection) — all in `rusm-otp`,
all without touching Wasm. See [links & supervision](./concepts/links-and-supervision.md).

## 2. With an already-compiled `.wasm` — embedding {#embed-wasm}

Add the `rusm-wasm` backend and host a prebuilt component as a process. A
`WasmRuntime` wraps an `rusm-otp` `Runtime`; **construct it inside a Tokio runtime**
(it starts the epoch ticker).

```rust
use rusm_otp::Runtime;
use rusm_wasm::{Capabilities, WasmRuntime};

#[tokio::main]
async fn main() {
    let rt = Runtime::new();
    let wasm = WasmRuntime::new(rt.clone()).unwrap();

    // compile once → prepare once (imports + entry export resolved) → spawn many.
    let bytes = std::fs::read("wasm/worker.wasm").unwrap();
    let prepared = wasm
        .prepare_component(&wasm.compile_component(&bytes).unwrap(), "run")
        .unwrap();

    // Default-deny Sandboxed profile…
    wasm.spawn_component(&prepared).join().await;

    // …or grant capabilities explicitly (here: an 8 MiB heap cap):
    let caps = Capabilities::nothing().max_memory(8 << 20);
    wasm.spawn_component_with(&prepared, caps).join().await;
}
```

A trap (or a denied capability the guest turns into a trap) exits the process
`Crashed`, so links and supervisors react exactly as for a native process. The
runnable [`host_components`](https://github.com/archan937/rusm/tree/main/examples/embedding/host_components)
example (`make example EX=host_components`) shows this end to end, including a
memory-cap denial.

> **Core modules.** A `wasm32-wasip1` core module works the same way with
> `compile` / `prepare(module, "run")` / `spawn` (see the wasip1 bridge).

## 3. With an already-compiled `.wasm` — the app model {#app-model}

For a RUSM-first project, declare components in `rusm.toml` and let the CLI load
and supervise them from `./wasm/`:

```toml
# rusm.toml
[node]
listen = "127.0.0.1:4000"
profile = "balanced"

[components.worker]      # loaded from ./wasm/worker.wasm
capability = "sandboxed" # a built-in or a custom profile (below)
resident = true          # long-lived service: boot-spawned + supervised
```

```sh
rusm run          # load every [components.<name>] from ./wasm/, register them, boot
                  # + supervise the resident ones
```

A component keyed `[components.<name>]` is always **registered** so a route or a sibling
can `spawn` it by name. `resident = true` additionally makes the node **boot-spawn** it
at startup and **supervise** it (auto-restart on crash, bounded by restart-intensity).
Without `resident`, it is spawned only on demand (a per-request handler, an on-demand
worker) — no idle parked instance.

**Serving on a real port.** To run a component as an HTTP / WS / SSE server, declare
a `[[serve]]` listener and run `rusm serve` — it binds each on its TCP `listen` address.
A `[[serve]]` entry is a **pure listener**: a routed HTTP/SSE listener names its handlers
in `[serve.routes]` (each a `[components.<name>]` entry that carries its own capability);
a WS or routes-less HTTP listener names its single handler with `component`. The fastest
way in is **`rusm new <name>`**, which scaffolds a ready-to-serve app (a zero-dependency
TS HTTP component, a `rusm.toml` with a `[[serve]]` entry, `.gitignore`, README):

```toml
[[serve]]
component = "api"         # the per-connection handler → ./wasm/api.{wasm,js}
protocol = "ws"           # "http" | "sse" | "ws"
listen = "127.0.0.1:8080"
```

```sh
rusm new hello && cd hello
rusm build
rusm serve
curl http://127.0.0.1:8080/
```

With `[log] level` at `info`+, `rusm serve` access-logs each served request —
`rusm http GET / → 200`, an SSE stream as `sse`, a WS upgrade as `ws … → 101` — in the
same stream as the lifecycle and guest logs.

**Custom capability profiles.** Beyond the three built-ins (`sandboxed` /
`network-client` / `trusted`), you can define your own — like Cargo's
`[profile.<name>]`. A profile `inherits` a built-in base (default `sandboxed`,
default-deny) and overrides only the grants it sets; a component references it by
name — and a node-registered component runs under **its own** declared profile,
whoever spawns it (the `allow-spawn` capability gates who may spawn; a guest can't fabricate
grants the operator never declared).

```toml
[capabilities.agent]
inherits = "network-client"   # base; omit for sandboxed (default-deny)
allow-spawn = true            # may spawn other components by name
max-memory-mb = 256
env = ["OPENAI_API_KEY"]      # grant these keys (values from process env / .env)
preopen = [{ host = "./data", guest = "/data", read-only = false }]

[components.pages-agent]
capability = "agent"          # resolves to the custom profile above
```

## 4. A Rust WASM component (source only) {#rust-component}

Write the source, let RUSM build it. A component lives under `components/<name>/`:

```
my-app/
├── rusm.toml
├── components/
│   └── worker/
│       ├── Cargo.toml      # crate-type = ["cdylib"], wit-bindgen
│       ├── wit/            # the rusm:runtime world (vendored from crates/rusm-wasm/wit)
│       └── src/lib.rs
└── wasm/                   # rusm build writes worker.wasm here
```

`src/lib.rs` binds the `rusm:runtime` actor world with `wit-bindgen` and exports
`run`:

```rust
wit_bindgen::generate!({ world: "process", path: "wit" });

use rusm::runtime::actor;

struct Component;

impl Guest for Component {
    fn run() {
        actor::set_label("worker");
        let msg = actor::receive();              // block for a message (bytes)
        actor::send(actor::own_pid(), &msg);     // echo to self, etc.
    }
}

export!(Component);
```

Build and run the whole app:

```sh
rusm build        # cargo build --target wasm32-wasip2 per components/* → ./wasm/
rusm run          # spawn them per rusm.toml
rusm dev          # build + run, then watch ./components and reload on edit
```

One toolchain, no jco, no cargo-component — `cargo build --target wasm32-wasip2`
componentizes directly. **`rusm dev`** keeps running: edit a component and save,
and it rebuilds + reloads it automatically (a dependency-free mtime watch).

## 5. A TS / JS WASM component (source only) {#ts-component}

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
`registerTag`/`killTag`/`whereisTag` ([process groups](concepts/process-management.md#process-groups-tags))/
`openStream`/`acceptStream`), the `spawn<T>()` typed client (call / `for await`
stream / callback args / `.cast` / `.stop()`), binary (`Uint8Array`) messages, and
[byte streams](#streaming-from-a-component) are all typed by the **`rusm-ts` package**.
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

## 6. Serve a component over HTTP (Rust, TypeScript, or Go) {#serve}

Any component can be a high-throughput **HTTP / WS / SSE** server — declare a
`[[serve]]` entry and run `rusm serve`. Serving is always **ephemeral**: HTTP/SSE run
a fresh sandboxed instance per request, WS one sandboxed process per connection. A
serving instance never holds state across requests — for that, run a long-lived
`[components.<name>]` service (`resident = true`) and reach it over the actor API
(`whereis` / `call`), or persist to the node `store` (`kv`). The fastest start is `rusm new <name>`; here is
the whole shape so you can copy and adapt it.

The layout (a Rust component shown; a TS one swaps `Cargo.toml` + `src/lib.rs` for a
single `index.ts`):

```text
my-api/
├── rusm.toml
├── .env                      # optional — env vars (the real process env always wins)
├── components/
│   └── api/
│       ├── Cargo.toml        # Rust  ·  or a single index.ts (TS)
│       └── src/lib.rs
└── wasm/                     # rusm build writes api.{wasm,qjsbc,js} here
```

`rusm.toml` — one `[[serve]]` listener hosts the component on a real port; its own
`[serve.routes]` subtable maps requests to handler actions (Rust only — TS handlers
dispatch themselves). The `[[serve]]` entry is a pure listener; the handler it routes to
is a `[components.<name>]` entry that carries its own capability:

```toml
[[serve]]                    # a pure listener — no name, no capability
protocol = "http"            # http | sse | ws
listen = "127.0.0.1:8080"

[serve.routes]               # this listener's own routes
"GET /" = "api#home"               # → the `home` action in the `api` component
"GET /users/:id" = "api#show"      # :id is a path param, read from `Params`

[components.api]             # the handler the routes name → ./wasm/api.{wasm,qjsbc,js}
capability = "sandboxed"     # carries its own capability (spawned per request)
```

The handler — same job, your language.

::: code-group

```rust [components/api/src/lib.rs]
// A routed Rust HTTP component: each `pub fn` is an action named in `[serve.routes]`.
// No `main`, no router — routing is declarative config. The macro hides the
// world, `Guest`, and `export!`.
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn home(_req: Request, _p: Params) -> Response {
        Response::text("Hello from RUSM 👋\n")
    }

    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }
}
```

```ts [components/api/index.ts]
// A web-standard TS HTTP handler — a `wasi:http` per-request component. It does
// its own dispatch, so no `[serve.routes]` table is needed.
export default function handle(request: Request): Response {
  const { pathname } = new URL(request.url);
  if (pathname === "/") return new Response("Hello from RUSM 👋\n");
  return new Response("not found\n", { status: 404 });
}
```

```go [components/api/main.go]
// A routed Go HTTP component: each handler is an action named in `[serve.routes]`.
// No main, no router — routing is declarative config; the bindings live in the SDK.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	h.Handle("home", func(_ web.Request, _ web.Params) web.Response {
		return web.Text("Hello from RUSM 👋\n")
	})
	h.Handle("show", func(_ web.Request, p web.Params) web.Response {
		id := p.Get("id")
		if id == "" {
			id = "?"
		}
		return web.Text("user " + id + "\n")
	})
	h.Serve()
}
```

:::

Build and serve:

```sh
rusm build
rusm serve
# serving 1 endpoint(s):
#   api              http://127.0.0.1:8080
curl http://127.0.0.1:8080/            # Hello from RUSM 👋
curl http://127.0.0.1:8080/users/42    # user 42   (Rust, via the :id route)
```

To adapt: add more `[serve.routes]` entries and matching `pub fn`s; stream **SSE** with
`protocol = "sse"` and a `rusm_rs::sse::serve` handler (`open`/`message`/`close`, one
process per connection — the TS twin is `export default sse({ open, message, close })`,
the Go twin `web.Sse{ Open, Message, Close }.Serve()`); or serve
**WebSocket** with `protocol = "ws"` and a `rusm_rs::ws::serve` handler
(`open`/`message`/`close`) — one sandboxed process per connection (the TS twin is
`export default websocket({ open, message, close })` from `rusm-ts`; the Go twin is
`web.WebSocket{ Open, Message, Close }.Serve()`). See
[the serving model](./concepts/serving-model.md).

## Process management from inside a component {#process-management}

A component imports the `rusm:runtime/actor` interface and calls the Erlang
`Process` API directly — the same operations the host has:

::: code-group

```rust [Rust]
use rusm::runtime::actor;

let me = actor::own_pid();                 // self()
actor::register("worker");                 // name yourself in the registry
let who = actor::whereis("worker");        // look a name up → Option<pid>
let all = actor::list_processes();         // every live pid (find all)
let info = actor::info(me);                // Option<process-info>: links, label, mailbox depth…
let alive = actor::is_alive(some_pid);
actor::send(some_pid, &bytes);             // message-pass (bytes)
let incoming = actor::receive();           // block for the next message
actor::kill(some_pid);                     // terminate another process
actor::unregister("worker");
actor::set_label("worker#1");              // a human label for the observer
```

```ts [TypeScript]
import { Process } from "rusm-ts";

const me = Process.self;                    // self()
Process.register("worker");                 // name yourself in the registry
const who = Process.whereis("worker");      // look a name up → bigint | null
const all = Process.list();                 // every live pid (find all)
const info = Process.info(me);              // links, label, mailbox depth… | null
const alive = Process.isAlive(somePid);
Process.send(somePid, bytes);               // message-pass (bytes or text)
const incoming = await Process.receive();   // await the next message
Process.kill(somePid);                      // terminate another process
Process.unregister("worker");
Process.setLabel("worker#1");               // a human label for the observer
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

me := rusm.Self()                  // self()
rusm.Register("worker")            // name yourself in the registry
who, ok := rusm.Whereis("worker")  // look a name up → (Pid, bool)
all := rusm.List()                 // every live pid (find all)
info, ok := rusm.Info(me)          // (ProcessInfo, bool): links, label, mailbox depth…
alive := rusm.IsAlive(somePid)
rusm.SendBytes(somePid, bytes)     // message-pass (bytes)
incoming := rusm.ReceiveBytes()    // block for the next message
rusm.Kill(somePid)                 // terminate another process
rusm.Unregister("worker")
rusm.SetLabel("worker#1")          // a human label for the observer
```

:::

The runnable proof is the `actor-echo` test fixture, which drives **every** op
from inside a real component.

> **Spawn-from-guest is supported — capability-gated.** A component declared in
> `rusm.toml` can be `spawn`ed **by name** from inside another component (`spawn`
> in the actor ABI), so you get per-request workers and concealed typed clients —
> the Erlang model. It's default-deny (the `allow-spawn` capability gates *who* may spawn);
> a **node-registered** component runs under **its own manifest-declared profile**
> (what the manifest declares is what runs, whoever spawns it), so secrets stay scoped
> to the component that needs them — never the spawner. Components still find
> long-lived peers with `register`/`whereis` and talk with `send`/`receive`; a
> request/reply "callback" is just a message and a reply. See
> [components & the actor world](./concepts/components-and-the-actor-world.md).

> **From TS/JS.** The same operations are bridged to the `Process` global in the
> [js-runner](#_5-a-ts-js-wasm-component-source-only): `Process.self()`,
> `Process.list()`, `Process.send(to, msg)`, `Process.receive()`,
> `Process.register/whereis/isAlive/kill/setLabel`,
> `Process.registerTag/killTag/whereisTag` (process groups).

## Streaming (from a component) {#streaming}

Cross-process **byte streams** are Tokio-backpressured and ride the mailbox as
`Received::Stream` — see [byte streams](./concepts/byte-streams.md). A component
opens a stream to **another process** — `peer` below, identified by its pid (from
`spawn`) or a name it `register`ed — writes chunks (the write parks under
back-pressure when the reader is slow), and closes it; the other side accepts and
reads to end-of-stream:

::: code-group

```rust [Rust]
use rusm::runtime::actor;

// Producer: find the peer by the name it registered (or use a pid from `spawn`),
// open a stream to it, write chunks, then close.
let peer = actor::whereis("consumer").expect("consumer is registered");
if let Some(id) = actor::stream_open(peer) {
    actor::stream_write(id, b"hello!");   // false if the reader is gone
    actor::stream_close(id);              // signals end-of-stream
}

// Consumer: accept the next incoming stream, read to EOF.
let id = actor::stream_accept();          // blocks until a stream arrives
while let Some(chunk) = actor::stream_read(id) {
    // …handle chunk (Vec<u8>)…           // None == end-of-stream
}
```

```ts [TypeScript]
import { Process } from "rusm-ts";

// Producer: open a stream to the peer (a pid, or — as here — a registered name;
// `openStream` accepts either), write chunks, then close.
const out = Process.openStream("consumer");          // null if the peer is gone
if (out) {
  out.write(new TextEncoder().encode("hello!"));     // false once the reader is gone
  out.close();                                       // signals end-of-stream
}

// Consumer: accept the next incoming stream, read to EOF.
const inc = Process.acceptStream();
let chunk;
while ((chunk = await inc.read()) !== null) {         // read() is async; null == EOF
  // …handle chunk (Uint8Array)…
}
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

// Producer: find the peer by the name it registered (or use a Pid from Spawn),
// open a stream to it, write chunks, then close.
peer, _ := rusm.Whereis("consumer")
if out, ok := rusm.OpenStream(peer); ok { // ok == false if `peer` is gone
	out.Write([]byte("hello!"))           // false once the reader is gone
	out.Close()                           // signals end-of-stream
}

// Consumer: accept the next incoming stream, read to EOF.
inc := rusm.AcceptStream() // blocks until a stream arrives
for {
	chunk, ok := inc.Read() // ok == false at end-of-stream
	if !ok {
		break
	}
	_ = chunk // …handle chunk ([]byte)…
}
```

:::

The same ops are available to **wasip1 core modules** through the raw `rusm::*`
ABI, and the **stream-pipe** benchmark drives the underlying `StreamHandle` at
multiple GB/s.

> **Roadmap.** A native p3-typed `stream<u8>` in the WIT signature (instead of the
> handle-based ops above) is a future ergonomic layer; the handle-based API is the
> real, working one today.

## Capabilities & sandboxing {#capabilities}

Every process is default-deny. Named profiles set the baseline; the `Capabilities`
builder overrides per spawn:

```rust
use rusm_wasm::{Capabilities, CapabilityProfile};

CapabilityProfile::Sandboxed.capabilities();          // CPU + bounded heap only
Capabilities::nothing()                               // start from nothing…
    .max_memory(16 << 20)                             // …a 16 MiB ceiling
    .allow_network(true)                              // …outbound sockets
    .preopen("/srv/data", "/data", /* read_only */ true) // …a mounted dir
    .env("LOG", "info");                              // …an env var
```

Grants map onto standard WASI plus a `StoreLimiter` memory cap. A breach traps
*only that process*. See [permissions & sandboxing](./concepts/permissions-and-sandboxing.md).

## Observe a running node {#observe}

**Your app** — start it as an attachable node, then attach a REPL to watch its
live processes:

```sh
rusm node start           # hosts your rusm.toml components + a live attach endpoint
rusm attach               # stream the live process table; `detail off` for just counts
```

**The benchmark dashboard** — the visual observer + scenario runner (repo-only):

```sh
make dashboard            # the benchmark node + the React dashboard ("the money")
# or run them separately:  make node   then   make ui
```

The dashboard's **Observer** shows the live process count and per-tick activity;
each scenario panel also unfolds its real engine source so you can see exactly how
it's built.
