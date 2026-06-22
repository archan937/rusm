# The serving model

A RUSM component can be a high-throughput **HTTP / WebSocket / SSE** server. The host
owns the socket and the protocol; the guest just produces responses — in Rust,
[TypeScript](/deep-dive/guests), or Go. One decision shapes everything that follows:
serving is **always process-per-unit-of-work**. `rusm-otp` stays Wasm-free; all the
serving machinery (hyper, tungstenite, `wasi:http`) lives only in `rusm-wasm`.

## One shape, by design

Serving is **always process-per-unit-of-work** — there is no "resident" mode to choose:

- **HTTP / SSE** — a fresh, sandboxed WASM instance **per request**.
- **WS** — one sandboxed component process **per connection**.

This single choice buys properties you'd otherwise have to engineer around:

- **No head-of-line blocking, ever.** Each request gets its own instance, so a slow or
  blocking handler can't stall the next request.
- **Crash containment.** A trap fails *just that* request or socket — never the listener
  or the other clients. There is no shared instance to corrupt.
- **Total isolation.** Each instance has its own linear memory and its own default-deny
  [capability profile](/deep-dive/permissions-and-sandboxing).
- **Cheap.** Per-request instances ride the pooled spawn path (pooling allocator + CoW
  linear memory + precomputed export index), ~440k component spawns/sec; RSS tracks only
  live instances.

## Where state goes

The serving instance is stateless and disposable. Anything that must outlive a request
lives elsewhere — **never** in the serving instance:

- a **`[components.<name>]` service** (`resident = true`) — a long-lived, supervised,
  stateful process reached
  over the actor API (`whereis` / `call` / `send`): a counter, cache, session map,
  pub/sub hub, chat-room registry; or
- durable **`kv`** for state that must survive restarts.

This is where the old "resident vs per-call" decision now lives: a resident
`[components.<name>]` **service** holds state; a **worker** spawns per call. Serving components are always
per-request. A handler that needs state simply `call`s a service and shapes the reply.

## Declarative routing

Routing lives in a per-listener `rusm.toml` **`[serve.routes]`** subtable — never in
handler code — and applies to **every protocol**: it matches by method + path. An `http`
listener dispatches each request to a `component#action`; an `sse`/`ws` listener routes the
**connection** to a **bare handler component** (no `#action` — the component is the
per-connection handler), capturing path params into its connection context. A listener with
no `[serve.routes]` binds a single handler by `component`. Each `[[serve]]` listener has its own
`[serve.routes]`, so multiple listeners (e.g. a public API and an admin port) route
independently. A key is `"METHOD /path/pattern"`; the value is `"component#action"` (HTTP)
or `"component"` (ws/sse):

- `:name` captures a path parameter (read via `Params::get("name")`);
- a trailing `*` captures the remaining segments;
- the separator is `#` (`:` and `.` are reserved by RUSM's scheme/file conventions).

Matching is by specificity (literal > `:param` > `*`). A path that matches but not for
the method → **405**; no match → **404**.

## Handlers are named actions

A Rust serving component is a module of `pub fn`s under `#[rusm_rs::handlers]` — no
`main`, no router, no wire plumbing. The macro generates the whole component shell and
the action dispatch; the developer writes only handler functions. Each action is a
**buffered** `fn(Request, Params) -> Response`. (Server-Sent Events are a per-connection
[`sse::serve`](/build-an-app/serve-sse) handler — like WS — not a routed action.)

TypeScript serving uses web standards instead (the macro is Rust): HTTP
`export default` a `fetch`-shaped handler, SSE `export default sse({…})`; WS uses
`export default websocket({ open, message, close })`, one worker per connection.

## How it works

- **HTTP / SSE** ride `wasi:http`. The host gateway resolves the route, spawns the
  matched handler fresh, dispatches the action over the actor wire, and turns the reply
  into the response — buffered, or for SSE a chunked streamed body draining the guest's
  back-pressured byte stream (see [byte streams](/deep-dive/byte-streams)).
- **WebSocket** upgrades host-side; each inbound frame becomes a mailbox message, and
  replies go out through a Wasm-free **writer process** that owns the socket sink — one
  isolated process per connection.
- An ephemeral Wasm-free **responder** process owns the reply hand-off so the sandboxed
  guest never touches a socket.
- **Standards-first:** a stock `wasi:http` component serves unchanged; the
  `rusm:runtime` actor world is opt-in.

## Serving and RPC unify

A serving handler and an actor-world service are the same thing — a component exporting
named functions. A handler **action** is reachable via an HTTP route; a service
**function** via an actor `call`. Same wire, same spawn model. So "shared state" is just
"a component you `call`."

## How it's benchmarked (honestly)

Serving throughput is measured **out-of-process** by the `rusm-loadtest` binary against
a real `rusm serve` port, so the load generator never steals the server's CPU and the
number is the server's — see the [benchmark reference](/about/benchmark-dashboard).

See the full [serving guide](/deep-dive/serving-http-ws-sse) for routing syntax, the
`#[rusm_rs::handlers]` macro, the `Sse` API, the TypeScript path, and a worked example;
the `[[serve]]` and `[serve.routes]` schema is in the
[configuration reference](/deep-dive/configuration).

For a complete runnable app in each language, see the collaborative todo board —
[TypeScript](https://github.com/archan937/rusm/tree/main/examples/typescript) ·
[Rust](https://github.com/archan937/rusm/tree/main/examples/rust) ·
[Go](https://github.com/archan937/rusm/tree/main/examples/go) — or scaffold one with
`rusm new <name> --template todo-board`.

> Phase 11. `rusm serve` hosts `rusm.toml [[serve]]` entries on real ports; 0.3.0 added
> native serving **TLS** (`https`/`wss` via `[serve.tls]`), so the serve path is no longer
> plaintext-only.
