# Concept — guests: Rust, TypeScript & Go

A RUSM process body can be written in **Rust** (`rusm-rs`), **TypeScript** (`rusm-ts`),
or **Go** (`rusm-go`). All compile/bundle to a sandboxed Wasm process with the same
actor API and the same JSON wire — so a Rust client, a TypeScript service, and a Go
worker interoperate transparently. Each SDK speaks its language's *own* idioms — the
concepts are shared, the surface is native.

## Rust guests (`rusm-rs`)

Ergonomic `Pid` / `send` / `receive` / `spawn` / registry / `Stream`, plus a
`#[rusm_rs::service]` macro that generates the receive → dispatch → reply loop **and**
a typed `Client` with call / cast / streaming / callbacks. `rusm build` compiles each
`components/<name>/` with `cargo build --target wasm32-wasip2` — one toolchain, no
cargo-component, no jco.

### Serving — `#[rusm_rs::handlers]`

A Rust serving component is a module of `pub fn(Request, Params) -> Response` under
`#[rusm_rs::handlers]` — **no `main`, no router, no wire plumbing.** The macro generates
the component shell and the action dispatch; the route is named in that listener's
[`[serve.routes]`](../serving-http-ws-sse.md) subtable as `"component#action"`.
(Server-Sent Events are a per-connection `rusm_rs::sse::serve` handler, not a routed
action — see below.)

```rust
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    // GET /users/:id  ->  "api#show"
    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }
}
```

WebSockets are **per connection**: implement `ws::Handler` and `ws::serve` it. The host
runs one isolated process per connection, hands each inbound frame to `message`, and you
reply through the `Connection`. `open` and `close` are optional — `close` fires once on
disconnect (clean or dropped), then the process exits:

```rust
use rusm_rs::ws::{self, Connection, Handler};

#[derive(Default)]
struct Echo;

impl Handler for Echo {
    fn open(&mut self, conn: &Connection) { conn.send(b"welcome\n"); }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) { conn.send(&data); } // echo
    fn close(&mut self, _conn: &Connection) {} // disconnect — clean or dropped
}

#[rusm_rs::main]
fn main() { ws::serve(Echo::default()); }
```

State that must outlive a request never lives in the serving instance — it goes in a
long-lived `[components.<name>]` service (`resident = true`) reached over the actor API
(`whereis` / `call` / `send`) or in durable `kv`.

## TypeScript guests (`rusm-ts`)

Import the `rusm-ts` package: a **service** is just exported functions, a **worker** is
`export default`. The *concealed typed client* makes `await svc.method(...)` read
like a local call — with `for await` streaming and callback arguments — while
`spawn` / `send` / `receive` stay hidden. `rusm build` bundles each component with
Bun into a small `.js`.

### Serving — web standards

TS serving uses **web standards** (the `#[handlers]` macro is Rust-only) and needs **no
`[serve.routes]` table** — the component *is* the handler. HTTP/SSE is `export default` a
`fetch`-shaped function returning a `Response`; SSE returns a streaming `ReadableStream`
body. WS is `export default websocket({ open, message, close })` from the `rusm-ts`
package — one worker process per connection:

```ts
// HTTP/SSE — a per-request wasi:http component
export default function handle(request: Request): Response {
  const who = new URL(request.url).searchParams.get("who") ?? "world";
  return new Response(`hello, ${who}\n`, { headers: { "content-type": "text/plain" } });
}
```

```ts
// WS — one worker per connection
import { websocket } from "rusm-ts";

export default websocket({
  open(s)        { s.send("welcome\n"); },
  message(s, d)  { s.send(d); },          // echo
});
```

## Go guests (`rusm-go`)

Write **normal Go**: the `rusm` package gives `Self` / `Send` / `Receive` / `Spawn` /
the registry / `Stream`, plus a `Service` with typed `Fn0`–`Fn3` handlers and a generic
`Call[R]` / `CallStream[R]` client; the `web` subpackage gives the serving surface.
Logging is the standard library (`log` / `log/slog`). There are no macros — the
component shell is a three-line `init` + `main` + `run`; the bindings live in the SDK.
`rusm build` compiles each `components/<name>/` with **TinyGo** to a `wasm32-wasip2`
component (no `wit/` dir, no bindings boilerplate in your source).

Serving mirrors Rust: HTTP is routed by `[serve.routes]` to named actions registered on a
`web.Handlers`; WebSocket and SSE are one process per connection ([`web.Sse`](./lifecycle-sse.md)).
An `Handle` action is a buffered `func(Request, Params) Response`:

```go
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	// GET /users/:id  ->  "api#show"
	h.Handle("show", func(_ web.Request, p web.Params) web.Response {
		return web.Text("user " + p.Get("id") + "\n")
	})
	h.Serve()
}
```

WebSocket is `web.WebSocket{ Open, Message, Close }.Serve()` — the host runs one isolated
process per connection, hands each inbound frame to `Message`, and you reply through the
`Conn`. `Open` and `Close` are optional — `Close` fires once on disconnect (clean or
dropped), then the process exits:

```go
func run() {
	web.WebSocket{
		Open:    func(c web.Conn) { c.Send([]byte("welcome\n")) },
		Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo
		Close:   func(c web.Conn) {},                            // disconnect — clean or dropped
	}.Serve()
}
```

As with the other guests, shared state lives in a long-lived `[components.<name>]`
service or in durable `kv`, never in the serving instance.

## Beyond messaging — timers, storage, pub/sub, crypto

All guests get more than `send`/`receive`, all over the same capability-gated ABI:

- **Timed receive** — `receive_timeout(ms)` (RS) / `Process.receive(ms)` (TS) /
  `rusm.ReceiveBytesTimeout(ms)` (Go): Erlang's `receive … after` — the next message,
  or `null`/`None`/`ok=false` on the deadline. The basis for heartbeats and any
  time-bound wait, with no busy loop.
- **Durable storage** — `rusm_rs::kv` (RS) / the `kv` global (TS) /
  `rusm.OpenBucket(...)` (Go): bucketed
  `get`/`set`/`delete`/`exists`/`list` over the node's embedded store
  (`rusm-kv`/redb), gated by the **storage** capability. Survives a restart, no
  external daemon. (TS bundles can also `import` npm — `@noble/*`, etc.)
- **Pub/sub fan-out** — `rusm_rs::pubsub::Topics`: keyed subscriber tracking +
  fan-out + **monitor-based pruning** of dead subscribers (the broker *mechanics* as
  a primitive, so app code carries none of it). Pairs naturally with
  [SSE serving](../serving-http-ws-sse.md): a per-request SSE action subscribes to a
  topic and live-tails it via `Sse::run`, and the monitor prunes the subscriber when the
  request process exits on disconnect.
- **Outbound `fetch` + Web Crypto** (TS): a capability-gated streaming `fetch`, and
  a native **`crypto.subtle`** (RustCrypto: SHA digest, HMAC sign/verify, AES-GCM) —
  so the Anthropic SDK and JWT/signing libraries work inside the sandbox.

## Logging from a component

There's nothing new to learn — **use each language's standard logging**, and the host
routes it to the node's log stream. The `log` crate (Rust), `console.*` (TS), and `log` /
`log/slog` (Go) all flow to the platform **`log` op**, which stamps the timestamp, this
process's `component#pid`, and the severity. There's no init to call and nothing to wire —
name, pid, and format are the platform's. Output is gated by the node's `[log] level`
([configuration](../reference-configuration.md#log--platform-lifecycle-logging)), the
single source of truth (not a capability) — a record below the threshold is dropped.

::: code-group

```rust [Rust]
// The standard `log` crate facade → the node's log stream (the entry-point macros
// install the platform logger; no init). The host adds time, component#pid, and level.
log::info!("handled {} in {}ms", id, elapsed);
log::warn!("retrying ({attempt})");
log::error!("gave up");
```

```ts [TypeScript]
// The web-standard console → the node's log stream (the runner routes it; no setup).
console.log("handled", id, "in", elapsed, "ms");
console.warn("retrying", attempt);   // → [warn] retrying 2
console.error("gave up");            // → [error] gave up
// Pids (bigint) and objects are stringified/JSON'd for you.
```

```go [Go]
// The standard log / log/slog packages — the rusm-go SDK routes them to the node's
// log stream (the host stamps time, component#pid, and severity). No setup, no wiring.
slog.Info("handled", "id", id, "ms", elapsed)
slog.Warn("retrying", "attempt", attempt)
log.Printf("gave up after %d attempts", attempt)
```

:::

These are your **application** logs. They're distinct from the **platform** lifecycle
log (`[log] level = …` → `rusm spawn/exit component#pid …`, [configuration](../reference-configuration.md#log--platform-lifecycle-logging)),
which the runtime emits and tags `rusm` so the two are easy to tell apart on stderr.

## The shared runner — tiny TS components (vs jco)

A TypeScript component is **just its bundle** running on **one shared ~920 KB
rquickjs runner**: the JS engine is compiled once and shared by *every* TS process.
Contrast jco / ComponentizeJS, which bakes a multi-megabyte JS engine (StarlingMonkey)
into **every** component. Ship 50 TS components and RUSM ships the engine **once**,
not fifty times — far smaller and saner.

It's also the only option that keeps a JS guest **inside the Wasm sandbox**: rquickjs
compiles to `wasm32-wasip2`, so a TS guest gets the same memory isolation,
[capabilities](./permissions-and-sandboxing.md) and [preemption](./epoch-preemption.md)
as a Rust one. (A native engine like V8/`deno_core` can't run inside a component.)

## Bytecode precompile

`rusm build` precompiles each bundle to **version-locked QuickJS bytecode**
(`wasm/<name>.qjsbc`); the runner loads it straight into the VM, skipping the parser
on cold start. Full JS + npm is kept — the engine is shared, not embedded per
component.

> Phase 8 (the `rusm-rs` / `rusm-ts` SDKs + the rquickjs runner); bytecode precompile
> is a later optimization on the same shared runner.
