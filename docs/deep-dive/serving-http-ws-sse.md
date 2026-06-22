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
> | **Go** | ✅ `web.NewHandlers()` actions | ✅ `web.WebSocket{ Open, Message, Close }` | ✅ `web.Sse{ Open, Message, Close }` |

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
  [capability profile](/deep-dive/permissions-and-sandboxing). One request cannot
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
  over the [actor API](/deep-dive/components-and-the-actor-world) (`whereis` / `call` /
  `send`). This is your counter, cache, session map, rate limiter, chat-room registry,
  pub/sub hub. A handler `call`s it and shapes the reply into a response.
- **Durable `kv`** — the embedded redb-backed key-value store, for state that must
  survive a restart (see the [configuration reference](/deep-dive/configuration)).

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

**It applies to every protocol — `http`, `sse`, and `ws`** — but the value shape differs
by serving model. A `http` listener matches each request by **method + path** and
dispatches to a `component#action` (per request). An `sse`/`ws` listener matches the
**connection's** path the same way, but the value is a **bare handler component** (no
`#action` — the component *is* the per-connection handler, one process per connection); the
captured path params reach it through its [connection context](#the-connection-context).

A listener with **no** `[serve.routes]` instead binds a single handler `component`
(spawned per request for HTTP, per connection for ws/sse). A path that matches no route is a
**404** (for ws/sse, the connection is refused before any upgrade).

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

### Per-connection routes (ws/sse)

An `sse`/`ws` listener routes the connection's path to a **bare handler component** (no
`#action`) and captures path params for it:

```toml
[[serve]]
protocol = "sse"
listen = "127.0.0.1:8081"

[serve.routes]
"GET /events/:plan/:collection/:id" = "events"   # the component IS the handler
"GET /stream/:app"                  = "stream"
```

Each connection spawns the matched component fresh; the captured params (`:plan`, …) are
read from the handler's [connection context](#the-connection-context).

## The connection context {#the-connection-context}

A per-connection WebSocket or SSE handler can read the request that opened it — method,
path, query, the captured route params, headers (e.g. `last-event-id`, `authorization`,
`origin`), the peer address, and any negotiated subprotocol. It's fixed for the
connection's life; read it once in `open`. The handler never parses the URL or headers
itself — the platform captures and delivers them.

| | Rust | TypeScript | Go |
|---|---|---|---|
| **handle** | `conn.info()` / `stream.info()` | `socket.info` / `stream.info` | `conn.Info()` / `stream.Info()` |
| path / query | `.path()` / `.query()` | `.path` / `.query` | `.Path()` / `.Query()` |
| route param | `.param("plan")` | `.param("plan")` | `.Param("plan")` |
| header | `.header("last-event-id")` | `.header("last-event-id")` | `.Header("last-event-id")` |
| method / addr | `.method()` / `.remote_addr()` | `.method` / `.remoteAddr` | `.Method()` / `.RemoteAddr()` |

A per-connection SSE handler that streams one entity's patches, picked by the route:

::: code-group

```ts [TypeScript]
export default sse({
  open(stream) {
    const plan = stream.info.param("plan");
    // subscribe to this plan's topic; replay from stream.info.header("last-event-id"), …
  },
  message(stream, patch) { stream.data(patch); },
});
```

```rust [Rust]
impl sse::Handler for Events {
    fn open(&mut self, s: &Stream) {
        let plan = s.info().param("plan").unwrap_or_default();
        // subscribe to this plan's topic; replay from s.info().header("last-event-id"), …
    }
    fn message(&mut self, s: &Stream, patch: Vec<u8>) { s.data(&patch); }
}
```

```go [Go]
web.Sse{
    Open: func(s web.Stream) {
        plan := s.Info().Param("plan")
        // subscribe to this plan's topic; replay from s.Info().Header("last-event-id"), …
    },
    Message: func(s web.Stream, patch []byte) { s.Data(patch) },
}.Serve()
```

:::

## WebSocket frames — binary, text, and close

A WebSocket handler replies to its connection with frames. The default reply is a
**binary** frame; a handler can also send a **text** frame (what browsers expecting text
messages want) or **close** the connection with a status code + reason. All three across
RS/Go/TS:

| | Rust | TypeScript | Go |
|---|---|---|---|
| binary frame | `conn.send(&bytes)` | `socket.send(bytes)` | `conn.Send(bytes)` |
| text frame | `conn.send_text("…")` | `socket.sendText("…")` | `conn.SendText("…")` |
| close (code, reason) | `conn.close(1000, "bye")` | `socket.close(1000, "bye")` | `conn.Close(1000, "bye")` |

::: code-group

```ts [TypeScript]
export default websocket({
  message(socket, frame) {
    socket.sendText(render(frame)); // a text frame the browser reads as a string
  },
});
```

```rust [Rust]
impl ws::Handler for Chat {
    fn message(&mut self, conn: &Connection, frame: Vec<u8>) {
        conn.send_text(&render(&frame)); // a text frame the browser reads as a string
    }
}
```

```go [Go]
web.WebSocket{
    Message: func(c web.Conn, frame []byte) {
        c.SendText(render(frame)) // a text frame the browser reads as a string
    },
}.Serve()
```

:::

`send_text`/`sendText`/`SendText` returns `false` if the socket has already closed.

**Subprotocol.** A `[[serve]]` WebSocket listener may declare `subprotocols = ["graphql-ws",
…]`; the host negotiates the first client-offered one present, echoes it in the `101`, and
surfaces it on the connection context (`info().subprotocol()`).

**Keep-alive.** An idle connection gets a periodic server **ping** (default 30s), so
idle-reaping proxies don't drop it; inbound client pings are auto-ponged by the platform.

**Inbound frames & back-pressure — by design.** An inbound frame reaches the handler as
**bytes**; the handler interprets it per its own protocol (a WS protocol is single-type in
practice — JSON-as-text or a binary codec — so a per-frame text/binary tag would be noise,
and adding one can't be done without breaking the additive guarantee). The **text/close**
path is explicitly back-pressured (a bounded per-connection channel — a handler outrunning a
slow client parks); **binary** frames flow through the writer's mailbox, back-pressured by
the socket itself (the writer awaits each `sink.send`). This is the same shape as SSE.

## Resource & security controls {#resource-security-controls}

A `[[serve]]` listener may bound what it accepts — all optional, all default-off (so
existing manifests are unaffected):

```toml
[[serve]]
component = "chat"
protocol = "ws"
listen = "127.0.0.1:8080"
max_connections = 10000                       # cap concurrent connections (any protocol)
max_message_size = 1048576                    # cap an inbound frame in bytes (WS)
allowed_origins = ["https://app.example.com"] # restrict the handshake Origin (WS, CSWSH)
```

- **`max_connections`** *(http · sse · ws)* — the most connections the listener serves at
  once. At the cap a new connection is **dropped before the handshake/stream opens**, so a
  flood can't pile up unbounded handler instances; a freed slot is reused as connections
  close. `None` (default) = unlimited.
- **`max_message_size`** *(ws)* — the largest inbound frame, in bytes. A larger frame
  **closes the connection** instead of allocating it. `None` (default) = the transport's
  own limit.
- **`allowed_origins`** *(ws)* — the `Origin` header values permitted on the handshake —
  **cross-site WebSocket hijacking (CSWSH) protection**. A handshake from an unlisted (or
  absent) `Origin` is refused with **`403`**, before any process is spawned. Empty
  (default) = any origin (no check). (A browser still applies CORS to HTTP/SSE replies via
  [`[serve.headers]`](/deep-dive/configuration#serve-headers-per-listener-response-headers);
  `Origin` checks are the WebSocket equivalent, since WS has no CORS preflight.)

## Compression {#compression}

Set `compression = true` on a `[[serve]]` listener (default off) to compress eligible
replies the client accepts — the platform handles it at the transport edge, so guest code
never sees it:

```toml
[[serve]]
component = "api"
protocol = "http"
listen = "127.0.0.1:8080"
compression = true
```

- **HTTP** *(routed `#[handlers]`)* — a buffered reply is **gzip**-compressed when the
  client sends `Accept-Encoding: gzip`, the content type is compressible (`text/*`, JSON,
  XML, SVG, `+json`/`+xml`), the body clears a ~256-byte threshold, and it carries no
  `content-encoding` already. The response gains `content-encoding: gzip` + `vary:
  accept-encoding`.
- **SSE** — the `text/event-stream` body is **gzip**-streamed, flushed per event (so
  nothing buffers waiting for more) with the gzip footer on close. Same `Accept-Encoding`
  gate.
- **WebSocket** — **permessage-deflate** (RFC 7692) is negotiated when the client offers
  it; each message is compressed standalone (no-context-takeover both directions, which
  bounds memory). A `max_message_size` also caps the *decompressed* size, guarding against
  deflate bombs.

The handler-less **`wasi:http`** path (a TS `export default { fetch }`) owns its own
response — which may stream — so it sets its own `content-encoding`; platform compression
covers the paths the platform itself frames (routed HTTP, SSE, WS).

## TLS — `https` / `wss` {#tls}

Add a [`[serve.tls]`](/deep-dive/configuration#servetls-listener-tls) cert/key to a listener
and it serves over TLS — `https` for HTTP/SSE, `wss` for WebSocket. The host terminates TLS on
each connection *before* HTTP, so routing, the connection context, compression, caps, and the
per-connection process model are all unchanged; only the transport is encrypted.

```toml
[[serve]]
component = "api"
protocol = "http"
listen = "0.0.0.0:8443"

[serve.tls]
cert = "certs/server.pem"
key  = "certs/server.key"
```

rustls + ring (the same stack as the cluster transport); the handshake runs in the
per-connection task, off the accept loop, so a slow client can't stall new connections. A
bad cert/key path fails `rusm serve` at startup rather than serving plaintext.

## Writing a handler

The handler is **just your code** — no router, no `main`, no wire/JSON plumbing; the platform
owns all of it. The only difference by language is how routing is wired. **Rust & Go are
routed**: a module/registry of named actions that `[serve.routes]` dispatches to (the
`#[rusm_rs::handlers]` macro even generates the whole Rust component shell — the `process`
world, the `Guest` impl, `export!`). **TypeScript is self-routing**: one `export default`
`fetch` that does its own dispatch, so it needs no `[serve.routes]` for HTTP.

::: code-group

```ts [TypeScript]
// self-routing — one fetch handler, no [serve.routes] table
export default async function handle(req: Request): Promise<Response> {
  const url = new URL(req.url);
  const m = url.pathname.match(/^\/users\/(.+)$/);
  if (m) return new Response(`user ${m[1]}\n`);                 // GET /users/:id
  if (req.method === "POST" && url.pathname === "/users")
    return new Response(await req.text(), { status: 201 });     // POST /users — read body
  return new Response("not found\n", { status: 404 });
}
```

```rust [Rust]
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

```go [Go]
func run() {
    h := web.NewHandlers()
    // GET /users/:id   ->   "api#show"
    h.Handle("show", func(_ web.Request, p web.Params) web.Response {
        return web.Text("user " + p.Get("id") + "\n")
    })
    // POST /users      ->   "api#create"  — read the request body
    h.Handle("create", func(req web.Request, _ web.Params) web.Response {
        return web.Bytes(201, req.Body).Header("content-type", "application/json")
    })
    h.Serve()
}
```

:::

For the routed languages the value `"api#show"` names handler `api`, action `show`; each
action is a **buffered** `(Request, Params) -> Response` that computes a complete reply the
host turns into the HTTP response (and can read the request body and set status/headers).
(Server-Sent Events are **not** a routed action — they are a per-connection handler, like
WebSocket; see [SSE](#sse-a-per-connection-handler).)

### Captured path parameters

A routed handler reads a `:name` segment (or the `*` wildcard tail) from the `Params` the
host captured — there's no URL parsing in your code. (TypeScript self-routes, so a TS handler
reads params straight from `new URL(request.url)` instead of a `Params` map.)

::: code-group

```rust [Rust]
// route: "GET /users/:id/posts/:post" = "api#post"
pub fn post(_req: Request, p: Params) -> Response {
    let user = p.get("id").unwrap_or("?");
    let post = p.get("post").unwrap_or("?");
    Response::text(format!("post {post} by user {user}\n"))
}
```

```go [Go]
// route: "GET /users/:id/posts/:post" = "api#post"
h.Handle("post", func(_ web.Request, p web.Params) web.Response {
    return web.Text("post " + p.Get("post") + " by user " + p.Get("id") + "\n")
})
```

:::

### SSE — a per-connection handler {#sse-a-per-connection-handler}

Server-Sent Events are served like WebSocket — **one sandboxed process per connection**. A
`protocol = "sse"` listener either names one handler `component` or routes by path via
`[serve.routes]` (the [per-connection routes](#per-connection-routes-wssse) above). The
handler subscribes to an event source in `open` (typically a **process-group tag**), emits
each pushed event in `message`, and cleans up in `close`:

::: code-group

```ts [TypeScript]
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream)        { Process.registerTag("todos"); }, // subscribe
  message(stream, ev) { stream.data(ev); },              // a published event → emit
  close(stream)       {},
});
```

```rust [Rust]
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

```go [Go]
web.Sse{
    Open:    func(s web.Stream) { rusm.RegisterTag("todos") }, // subscribe
    Message: func(s web.Stream, ev []byte) { s.Data(ev) },     // a published event → emit
    Close:   func(s web.Stream) {},
}.Serve()
```

:::

A publisher broadcasts to the tag — `whereis_tag("todos")` then `send` each pid — and
every open stream's `message` fires (push, not polling). The **platform** owns the SSE
wire (the `text/event-stream` head + `Cache-Control: no-cache`, `data:` framing, keep-alive
heartbeats), the **bounded, back-pressured** body, and disconnect (the body's writer dies →
`close` fires), so a slow client slows the producer instead of growing memory and an idle or
endless feed never leaks. See the [SSE lifecycle](/build-an-app/serve-sse) and
[byte streams](/deep-dive/byte-streams).

#### Rich events & resumption

Beyond the plain `data:` shortcut, a handler emits **rich events** with an `event:` type, an
`id:` (the basis for resumption), and a `retry:` reconnect hint:

| | Rust | TypeScript | Go |
|---|---|---|---|
| plain data | `s.data(&bytes)` | `s.data(bytes)` | `s.Data(bytes)` |
| rich event | `s.emit(&Event { data, id: Some("42"), event: Some("tick"), ..Default::default() })` | `s.emit({ data, id: "42", event: "tick" })` | `s.Emit(web.Event{ Data, ID: "42", Name: "tick" })` |

**Resumption (`Last-Event-ID`).** Emit an `id:` with each event; when a dropped client
reconnects, the browser sends the last id it saw as the `Last-Event-ID` header, which the
handler reads from its [connection context](#the-connection-context) and replays from:

::: code-group

```ts [TypeScript]
open(stream) {
  const from = stream.info.header("last-event-id"); // null on first connect
  for (const ev of eventsSince(from))               // replay the gap, then live-tail
    stream.emit({ data: ev.data, id: ev.id });
}
```

```rust [Rust]
fn open(&mut self, s: &Stream) {
    let from = s.info().header("last-event-id"); // None on first connect
    for ev in events_since(from) {               // replay the gap, then live-tail
        s.emit(&Event { data: &ev.data, id: Some(&ev.id), ..Default::default() });
    }
}
```

```go [Go]
Open: func(s web.Stream) {
    from := s.Info().Header("last-event-id") // "" on first connect
    for _, ev := range eventsSince(from) {   // replay the gap, then live-tail
        s.Emit(web.Event{Data: ev.Data, ID: ev.ID})
    }
},
```

:::

The rich-event path is bounded + back-pressured like `data:`; `id`/`event` are single-line
(an embedded newline is dropped, so framing can't be injected).

## `[[serve]]` — declaring a listener

A `[[serve]]` entry is a **pure listener**. It carries no handler and no capability of
its own — the handler components live in `[components.<name>]` (with their own
capability), and `[serve.routes]` names them. Its fields:

| Key | Meaning |
|---|---|
| `protocol` | `http` · `sse` · `ws`. |
| `listen` | TCP address to bind, e.g. `"127.0.0.1:8080"`. |
| `component` *(optional)* | The single handler component for a listener that has **no routes**: a **WS** listener, or a routes-less `wasi:http` **HTTP** listener (e.g. a TS `export default` fetch). A routed HTTP/SSE listener has **no `component`** — its `[serve.routes]` name the handlers instead. |

For **HTTP/SSE** with a `[serve.routes]` subtable, each request is resolved against that
listener's routes → the matched handler component (a `[components.<name>]` entry) is spawned
fresh under **its own** capability → the matched action is dispatched → its reply becomes
the HTTP response. A **WS** `[[serve]]` (or a routes-less HTTP one) runs its `component`
once per connection / request; that component's capability comes from a matching
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
[[serve]]                               # a routed listener — no component, no capability
protocol  = "http"                      # http | sse | ws
listen    = "127.0.0.1:8080"

[serve.routes]                          # this listener's own routes
"GET  /"               = "api#home"
"GET  /users/:id"      = "api#show"
"POST /users"          = "api#create"
"GET  /static/*"       = "api#static"   # wildcard tail

# The handler the routes name — declared in [components.<name>], carries its own
# capability; spawned per request, so no `resident`:
[components.api]                  # wasm/api.{wasm,js}
capability = "sandboxed"          # default-deny profile

# Shared state is NOT in the handler — it's a long-lived, resident service:
[components.sessions]             # a stateful GenServer-style process
capability = "sandboxed"
resident = true                   # boot-spawned + supervised
```

The handler (`components/api/`) — the routed languages register one action per route;
TypeScript self-routes one `fetch` (so it needs no `[serve.routes]`, just `component = "api"`):

::: code-group

```ts [TypeScript]
// components/api/index.ts — one self-routing fetch handler
export default async function handle(req: Request): Promise<Response> {
  const url = new URL(req.url);
  if (url.pathname === "/") return new Response("hello from RUSM\n");
  const u = url.pathname.match(/^\/users\/(.+)$/);
  if (u) return new Response(`user ${u[1]}\n`);
  if (req.method === "POST" && url.pathname === "/users")
    // For state, call the resident `sessions` service — never store it in this instance.
    return new Response(await req.text(), { status: 201 });
  const s = url.pathname.match(/^\/static\/(.+)$/);
  if (s) return new Response(`serving ${s[1]}\n`);
  return new Response("not found\n", { status: 404 });
}
```

```rust [Rust]
// components/api/src/lib.rs
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

```go [Go]
// components/api/main.go
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
		return web.Text("hello from RUSM\n")
	})
	h.Handle("show", func(_ web.Request, p web.Params) web.Response {
		return web.Text("user " + p.Get("id") + "\n")
	})
	h.Handle("create", func(req web.Request, _ web.Params) web.Response {
		// For state, call the resident `sessions` service — never store it here.
		return web.Bytes(201, req.Body)
	})
	h.Handle("static", func(_ web.Request, p web.Params) web.Response {
		return web.Text("serving " + p.Get("*") + "\n")
	})
	h.Serve()
}
```

:::

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
[guests guide](/deep-dive/guests).

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

A serving handler and an [actor-world service](/deep-dive/components-and-the-actor-world)
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
