# Serving HTTP, WS & SSE from a component (Phase 11)

> **Status: HTTP, WS, and SSE all work — from both Rust and TypeScript components,
> served on real ports by `rusm serve`.** Serving is **always
> process-per-unit-of-work**: a fresh sandboxed WASM instance per HTTP/SSE request, one
> sandboxed component process per WS connection. The **fair, credible headline
> numbers** are measured **out-of-process** by
> [`rusm-loadtest`](https://github.com/archan937/rusm/tree/main/bench/rusm-loadtest)
> against a live `rusm serve` port (loopback): HTTP **~46k req/s** at 0% errors, WS
> **~146k round-trips/s** across 256 held connections, SSE **~609k events/s** across
> 256 held streams, and **~34k sandboxed-process-per-connection WS
> establishments/sec** (`rusm-loadtest`'s `conn` mode — each connection spawns a full
> component). The `rusm-otp` core stays Wasm-free throughout (hyper,
> `tokio-tungstenite`, and `wasi:http` live only in `rusm-wasm`).
>
> | | HTTP | WS | SSE |
> |---|---|---|---|
> | **Rust** | ✅ `#[rusm_rs::handlers]` actions | ✅ `ws::serve` handler | ✅ `sse::serve` handler |
> | **TypeScript** | ✅ `export default` `fetch` handler | ✅ `websocket({ open, message, close })` | ✅ `sse({ open, message, close })` |

RUSM runs a component as a high-throughput **HTTP(S) / WS(S) / SSE server** — a
sandboxed, supervised handler answering requests. The whole serving model rests on one
decision, and everything else falls out of it.

## One model: process-per-unit-of-work

There is exactly **one** serving shape, and it is not negotiable per endpoint:

- **HTTP / SSE** — a **fresh, sandboxed WASM instance per request**.
- **WS** — **one sandboxed component process per connection**.

That is the whole model. There is no "resident" serving mode, no warm pool to
configure, no `mode` switch. The consequences are the point:

- **Head-of-line blocking is impossible by construction.** Requests don't queue behind
  each other on a shared instance — each gets its own. A handler that blocks for a
  second blocks only its own request.
- **A crash drops one unit of work, never the server.** A trap (panic, OOM, capability
  violation) in a handler fails *that one* request or *that one* connection. The
  listener keeps accepting; every other client is untouched. There is no shared mutable
  state to corrupt, because there is no shared instance.
- **Total isolation.** Each instance has its own linear memory and its own default-deny
  [capability profile](./concepts/permissions-and-sandboxing). One request cannot
  observe, corrupt, or starve another.
- **It's cheap.** Per-request instances ride RUSM's optimized spawn path — pooling
  allocator + copy-on-write linear memory + a precomputed export index — at **~440k
  component spawns/sec**. RSS tracks only the instances *currently live*, so idle
  capacity costs nothing.

The old objection to instance-per-request — "but I need state across requests" — is
answered by moving state to where it belongs, not by leaking it into an ephemeral
serving instance.

### Where shared / long-lived state lives

State that must outlive a single request goes in one of two places, **never** in the
serving instance:

- **A `[components.<name>]` service** (`resident = true`) — a long-lived, supervised,
  stateful process you reach
  over the [actor API](./concepts/components-and-the-actor-world) (`whereis` / `call` /
  `send`). This is your counter, cache, session map, rate limiter, chat-room registry,
  pub/sub hub. A handler `call`s it and shapes the reply into a response.
- **Durable `kv`** — the embedded redb-backed key-value store, for state that must
  survive a restart (see the [configuration reference](./reference-configuration)).

This is where the old "resident vs per-call" distinction now lives — and it lives in
`[components.<name>]`, not in serving: a `resident = true` **service** holds state and is
reached by name; a **worker** is spawned per call. Serving components themselves are always
stateless and per-request. The serving instance is the cheap, disposable front; the
service or `kv` is the durable back. Clean separation, no compromise on isolation.

> **What changed (migration).** Earlier RUSM had a `mode = "resident"` serving option
> with `instances` / `shard_by` / `max_inflight` and a `rusm_rs::http::{Handler,
> serve}` trait API. That is **gone**. A stateful handler becomes: a stateless serving
> component (the route handler) plus a `[components.<name>]` service (the state) it `call`s,
> or `kv` for durable state. The `[[serve]]` fields `mode`, `instances`, `shard_by`,
> and `max_inflight` are removed (unknown keys are now a hard config error).

## Declarative routing — `[serve.routes]`

Routing lives in a per-listener TOML **`[serve.routes]`** subtable — never in handler
code.

**It applies to `http` listeners only.** A `http` listener matches each incoming request
by **method + path** and dispatches to a `component#action`. **SSE and WebSocket do not
path-route** — each binds **one** handler component and spawns a fresh process of it **per
connection** (an inbound WS frame, or the SSE stream itself, → that process), so there is no
path → action table; to serve different SSE/WS endpoints, bind a separate `[[serve]]`
listener per endpoint. A `[serve.routes]` table on an `sse`/`ws` listener is ignored.

Each `[[serve]]` HTTP listener has its own `[serve.routes]`, so multiple
listeners (e.g. a public API on `:8080` and an admin port on `:9090`) route
independently. A key is `"METHOD /path/pattern"`; a value is `"component#action"`:

```toml
[[serve]]
protocol = "http"
listen = "127.0.0.1:8080"

[serve.routes]                                   # this listener's own routes
"GET  /"                       = "web#home"
"GET  /users/:id"              = "api#show"      # :id captures a path param
"POST /users"                  = "api#create"
"GET  /users/:id/posts/:post"  = "api#post"      # multiple params
"GET  /files/*"                = "files#serve"   # trailing * captures the tail
```

- **`:name`** captures one path segment as a parameter, read in the handler via
  `Params::get("name")`.
- A **trailing `*`** captures the remaining segments (one or more) as the `*` param —
  e.g. `/files/a/b/c` gives `*` = `"a/b/c"`.
- **The separator is `#`**, deliberately: `:` is taken by RUSM's scheme syntax (`kv:`,
  `url:`), and `.` reads like a file extension or a method call. `component#action`
  is unambiguous.

**Matching is by specificity:** a literal segment beats a `:param` beats a `*`. So with
both `GET /users/new` and `GET /users/:id` registered, `/users/new` resolves to the
literal route and `/users/42` to the param route. Resolution has three outcomes:

- a route matches the path **and** the method → dispatch to its `component#action`;
- a route matches the path but **not** the method → **HTTP 405 Method Not Allowed**;
- no route matches the path → **HTTP 404 Not Found**.

All of this is decided by the host gateway from config. The guest never sees a router.

## Handlers are named actions — no `main()`

A Rust serving component is a module of `pub fn`s under `#[rusm_rs::handlers]`. The
developer writes **only** the handler functions. There is no router, no `main`, no
wire/JSON plumbing — the macro generates the entire component shell (the `process`
world, the `Guest` impl, `export!`) and the action dispatch.

> **The TypeScript equivalent is web standards**, not this macro — a TS handler is an
> `export default` `fetch` / `websocket({…})` / `sse({…})` and does its own dispatch
> (no `[serve.routes]` for HTTP). See [TypeScript serving](#typescript-serving-web-standards)
> below for the matching HTTP, SSE, and WS forms.

```rust
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    // GET /users/:id   ->   "api#show"
    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }

    // POST /users      ->   "api#create"  — read the request body
    pub fn create(req: Request, _p: Params) -> Response {
        Response::new(201, req.body).header("content-type", "application/json")
    }
}
```

The route value `"api#show"` names module `api`, action `show`. Each action is a
**buffered** `fn(Request, Params) -> Response` — it computes a complete response and the
host turns it into the HTTP reply. (Server-Sent Events are **not** a routed action — they
are a per-connection handler, like WebSocket; see [SSE](#sse-a-per-connection-handler).)

### `Params` — captured path parameters

`Params::get(name)` returns the segment captured by `:name` (or `Some("a/b/c")` for the
`*` wildcard), `None` if the route had no such parameter:

```rust
pub fn post(_req: Request, p: Params) -> Response {
    let user = p.get("id").unwrap_or("?");
    let post = p.get("post").unwrap_or("?");
    Response::text(format!("post {post} by user {user}\n"))
}
```

### SSE — a per-connection handler {#sse-a-per-connection-handler}

Server-Sent Events are served like WebSocket — **one sandboxed process per connection**,
not a routed action. A `protocol = "sse"` listener names one handler component (no
`[serve.routes]`); the handler subscribes to an event source in `open` (typically a
**process-group tag**), emits each pushed event in `message`, and cleans up in `close`:

```rust
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;
impl Handler for Feed {
    fn open(&mut self, _s: &Stream) { rusm_rs::register_tag("todos"); } // subscribe
    fn message(&mut self, s: &Stream, ev: Vec<u8>) { s.data(&ev); }     // a published event → emit
    fn close(&mut self, _s: &Stream) {}
}

#[rusm_rs::main]
fn run() { sse::serve(Feed); }
```

A publisher broadcasts to the tag — `whereis_tag("todos")` then `send` each pid — and
every open stream's `message` fires (push, not polling). The **platform** owns the SSE
wire (the `text/event-stream` head + `Cache-Control: no-cache`, `data:` framing, keep-alive
heartbeats), the **bounded, back-pressured** body, and disconnect (the body's writer dies →
`close` fires), so a slow client slows the producer instead of growing memory and an idle or
endless feed never leaks. The TS twin is `export default sse({ open, message, close })`; the
Go twin is `web.Sse{ Open, Message, Close }.Serve()`. See the
[SSE lifecycle](./concepts/lifecycle-sse) and [byte streams](./concepts/byte-streams).

## `[[serve]]` — declaring a listener

A `[[serve]]` entry is a **pure listener**. It carries no handler and no capability of
its own — the handler components live in `[components.<name>]` (with their own
capability), and `[serve.routes]` names them. Its fields:

| Key | Meaning |
|---|---|
| `protocol` | `http` · `sse` · `ws`. |
| `listen` | TCP address to bind, e.g. `"127.0.0.1:8080"`. |
| `name` *(optional)* | The single handler component for a listener that has **no routes**: a **WS** listener, or a routes-less `wasi:http` **HTTP** listener (e.g. a TS `export default` fetch). A routed HTTP/SSE listener has **no `name`** — its `[serve.routes]` name the handlers instead. |

For **HTTP/SSE** with a `[serve.routes]` subtable, each request is resolved against that
listener's routes → the matched handler component (a `[components.<name>]` entry) is spawned
fresh under **its own** capability → the matched action is dispatched → its reply becomes
the HTTP response. A **WS** `[[serve]]` (or a routes-less HTTP one) runs the `name`d
component once per connection / request; that component's capability comes from a matching
`[components.<name>]` entry, else default-deny `sandboxed`.

So the model is a clean split: **`[[serve]]` = the listener; `[components.<name>]` = the
handler/service components (each with its capability); `[serve.routes]` ties them
together.**

## A full worked example

> **Prefer a complete, runnable app?** The whole model on this page — HTTP CRUD, a live
> SSE feed, WebSocket chat, and a service driven by a worker — is implemented as a
> *collaborative todo board* in all three guest languages:
> [TypeScript](https://github.com/archan937/rusm/tree/main/examples/typescript) ·
> [Rust](https://github.com/archan937/rusm/tree/main/examples/rust) ·
> [Go](https://github.com/archan937/rusm/tree/main/examples/go). Scaffold your own copy
> with `rusm new <name> --template todo-board --lang ts|rust|go`. The snippets below are
> the minimal form of what those apps wire together.

`rusm.toml`:

```toml
[[serve]]                               # a pure listener — no name, no capability
protocol  = "http"                      # http | sse | ws
listen    = "127.0.0.1:8080"

[serve.routes]                          # this listener's own routes
"GET  /"               = "api#home"
"GET  /users/:id"      = "api#show"
"POST /users"          = "api#create"
"GET  /static/*"       = "api#static"   # wildcard tail

# The handler the routes name — declared in [components.<name>], carries its own
# capability; spawned per request, so no `resident`:
[components.api]                  # wasm/api.wasm
capability = "sandboxed"          # default-deny profile

# Shared state is NOT in the handler — it's a long-lived, resident service:
[components.sessions]             # a stateful GenServer-style process
capability = "sandboxed"
resident = true                   # boot-spawned + supervised
```

`components/api/src/lib.rs`:

```rust
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    pub fn home(_req: Request, _p: Params) -> Response {
        Response::text("hello from RUSM\n")
    }

    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }

    pub fn create(req: Request, _p: Params) -> Response {
        // For state, `call` the `sessions` service via the actor API — never
        // store it in this ephemeral instance.
        Response::new(201, req.body)
    }

    pub fn static_(_req: Request, p: Params) -> Response {
        Response::text(format!("serving {}\n", p.get("*").unwrap_or("")))
    }
}
```

```sh
rusm build           # cargo wasm32-wasip2 per components/*
rusm serve           # binds 127.0.0.1:8080
curl http://127.0.0.1:8080/users/42      # -> user 42
```

Start from a scaffold with **`rusm new <name>`** (a zero-dependency TS HTTP component,
a `rusm.toml` `[[serve]]` entry, `.gitignore`, README):

```sh
rusm new hello && cd hello && rusm build && rusm serve
curl http://127.0.0.1:8080/
```

## TypeScript serving — web standards

TypeScript serving uses **web standards** (the `#[handlers]` macro is Rust-only). TS
HTTP/SSE components run on the embedded rquickjs **js-http-runner** — a raw-`wasi:http`
component instantiated per request — and need **no `[serve.routes]` table**; the component
*is* the handler. The runner is **wizer-pre-initialized**: the QuickJS engine + the Web-API
bridge are booted once at build time and snapshotted into the image, so each per-request
instance starts *warm* and only evaluates your bundle + runs `fetch` (≈8× the cold
per-request rate) — still a fresh, isolated instance per request, never resident.

**HTTP** — `export default` a request → response function:

```ts
export default function handle(request: Request): Response {
  const who = new URL(request.url).searchParams.get("who") ?? "world";
  return new Response(`hello, ${who}\n`, {
    headers: { "content-type": "text/plain" },
  });
}
```

(The Workers/Deno `export default { fetch }` shape is also accepted, so those components
port over.)

**SSE** — `import { sse }` and export a per-connection handler set (the SSE twin of
`websocket`); one TS process runs per connection. Subscribe to an event source in `open`
(a process-group tag), emit each pushed event in `message` — the platform owns the
`text/event-stream` wire, heartbeats, and disconnect:

```ts
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream)        { Process.registerTag("todos"); },  // subscribe
  message(stream, ev) { stream.data(ev); },               // a published event → emit
  close(stream)       { /* disconnect — clean or dropped */ },
});
```

**WS** — `import { websocket }` and export a per-connection handler set; one TS worker
process runs per connection (no pids, no manual mailbox):

```ts
import { websocket } from "rusm-ts";

// One isolated process per connection, so reply to *this* socket here. A module-level
// array would NOT broadcast — each connection has its own. To fan out across connections
// use process-group tags (`Process.registerTag`/`whereisTag`); see the chat example.
export default websocket({
  open(s)       { s.send("welcome\n"); },
  message(s, d) { s.send(d); },   // echo this connection's frame
  close(_s)     { /* disconnect — clean or dropped */ },
});
```

(The lower-level worker shape — `export default async function ()` that receives the
writer pid as message 1, then echoes frames — is also available; `websocket({…})` is
the ergonomic wrapper over it.)

Every guest stays sandboxed (a serving component gets only the capabilities its profile
grants) and supervised (a crash restarts the handler, never the listener). See the
[guests guide](./concepts/guests).

## How the host gateway works (platform code)

None of this is visible to the app author — it all lives in `rusm-wasm`:

1. The listener accepts a connection (process-per-connection TCP; **HTTPS/WSS**
   terminate with the same rustls stack as the cluster, once wired).
2. **HTTP/SSE:** the gateway resolves the request against that listener's compiled `[serve.routes]` table
   (`RouteTable::resolve` → matched `component#action` + captured params, or 405/404),
   spawns the matched handler component fresh on the optimized spawn path, and
   dispatches the action over the JSON actor wire (request body base64-encoded).
3. An ephemeral **Wasm-free "responder" process** owns the reply hand-off: the handler's
   reply comes back over a `oneshot`, and the responder turns it into the HTTP response
   — **buffered**, or for SSE a chunked **streamed** body that drains the guest's
   back-pressured byte stream directly into the response.
4. **WS:** hyper surfaces the `Upgrade`, `tokio-tungstenite` runs the protocol
   (handshake, masking, ping/pong, fragmentation, close), and the named component runs
   once per connection — each inbound frame becomes a mailbox message; replies go out
   through a Wasm-free **writer process** that owns the socket sink. The guest never
   touches a socket or raw frames.

The guest contract is the standard `wasi:http` WIT (HTTP/SSE) plus RUSM's actor wire;
WS is a host-side convention (there is no WASI WS standard to be non-portable against).

## Serving components and RPC services unify

A serving handler and an [actor-world service](./concepts/components-and-the-actor-world)
are the **same thing**: a component exporting named functions. A handler **action** is
reachable via an HTTP route; a service **function** is reachable via an actor `call`.
Same wire, same spawn model. That is why "shared state" is just "another component you
`call`" — there is one composition primitive, used two ways.

## Battle-proven foundations (no reinvention)

- **hyper** — HTTP/1.1 + HTTP/2 parsing and connection management.
- **`wasmtime-wasi-http`** — the official hyper ↔ `wasi:http` bridge (we hand-roll the
  same host interface where the off-the-shelf crate falls short, e.g. p3 streaming
  bodies — the guest's `wasi:http` contract is fixed either way).
- **`tokio-tungstenite`** — the battle-proven WebSocket protocol; the host runs it, the
  guest sees clean messages.
- **Web `Request`/`Response`/`ReadableStream`** — the Workers/Deno shape for TS.
- **rustls + ring** — HTTPS/WSS termination, the same stack as the cluster.
- **RUSM's own** — the pooled instance-per-request spawn path, Tokio-back-pressured byte
  streams, the on-demand overflow tier (so thousands of concurrent SSE/WS streams aren't
  capped by a fixed pool — they spill to the on-demand engine, bounded by RAM), bounded
  mailboxes for per-connection back-pressure, capability profiles, and supervision.

## Benchmarks

Serving is benchmarked the **fair** way — **out-of-process**, by
[`rusm-loadtest`](https://github.com/archan937/rusm/tree/main/bench/rusm-loadtest)
against a real `rusm serve` port. The load generator runs in a separate process (never
sharing the server's CPU) and crosses a real socket.

- **HTTP** uses the **balter** crate (a Tokio-native load framework) as a **fixed-rate
  sweep**: drive increasing target req/s and, at each level, measure achieved
  throughput + tail latency + error rate, climbing until the SLA breaks or throughput
  plateaus. (balter's auto-saturation loop is too cautious in the sub-millisecond
  loopback regime, so we drive its constant-rate controller and sweep ourselves — every
  number is a direct measurement, none extrapolated.)
- **WS & SSE** use a tokio-native **connection-capacity harness** (held connections
  sustaining echo round-trips / draining events) — these are connection-capacity
  workloads, not request-rate.
- **`conn`** is a connection-establishment storm: fresh WS connections opened as fast as
  the server accepts them — each spawning a full sandboxed component process, a richer
  claim than a raw TCP accept rate.

Measured out-of-process (loopback):

| Topic | Method | Measured |
| --- | --- | --- |
| **HTTP** | balter fixed-rate sweep | **~46k req/s at 0% errors.** |
| **WS** | connection-capacity harness | **~146k round-trips/s across 256 held connections.** One sandboxed process per connection; the per-message writer→component→writer hop costs ~nothing. |
| **SSE** | connection-capacity harness | **~609k events/s across 256 held streams.** A dropped client tears down only its own instance. |
| **Connections** | `conn` establishment storm | **~34k sandboxed-process-per-connection WS establishments/sec.** Each connection spawns a full component. |

The dashboard also carries **six co-resident serving demo tiles** (`http-throughput`,
`ws-echo`, `sse-fanout` and their `*-ts` twins): each spins up the same real in-process
WASM server and drives it through the **same load path** as `rusm-loadtest` (balter for
HTTP request-rate, the connection-capacity harness for WS/SSE), with load generator and
server sharing the node process. They are honest **live demos** — useful to watch a real
server take load — but because they share CPU and hide the network behind loopback,
their figures (http-throughput ~20k req/s, ws-echo ~195k rt/s, sse-fanout ~695k events/s)
differ **by design** from the fair out-of-process headlines above, which remain the
source of truth for *served* throughput.

What "good" looks like, confirmed: HTTP serving thousands of isolated
instance-per-request handlers a second over a real socket at zero errors; WS/SSE holding
every connection open under load (bounded by RAM, not a fixed cap); latency flat because
the streams are Tokio-back-pressured.
