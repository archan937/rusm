# Phase 11 — Serving & the standard-WASI surface

Phase 11 turns RUSM from a runtime that *hosts* components into a server you can put on a port. A component becomes a real HTTP, WebSocket, or SSE endpoint — written in Rust, TypeScript, or Go — and `rusm serve` hosts it on real TCP. It also closes the standard-WASI surface, so stock command components and real npm clients run unchanged.

## Why this phase

Everything through Phase 10 was reachable only by embedding RUSM in Rust or driving it from the dashboard. To be a platform people actually deploy, RUSM has to serve real web traffic — and it has to do so without betraying the lifecycle model that is the whole point of the project.

That ruled out the obvious design. A single resident request handler is the conventional choice, and it is exactly wrong here: one slow request blocks the line behind it, and one crash takes down every in-flight request with it. RUSM already had cheap processes and true per-process isolation — so serving should *use* them. The question Phase 11 answers is "what does serving look like when a fresh sandboxed process per unit of work is affordable?"

The second half of the phase is less glamorous but just as load-bearing: support the standard WASI surface (`wasi:http` in *and* out, `wasi:cli/run`, `wasi:random`) so that real components — and the npm clients real apps depend on — run without bespoke shims.

## What shipped

1. **HTTP serving** — `WasmRuntime::http_server` runs a **fresh `wasi:http` instance per request** (via `ProxyPre`), on the optimized spawn path. A crash drops exactly one request; there is no resident handler to block the line.
2. **WebSocket serving** — `ws_server` runs **one sandboxed component process per connection**. Inbound frames arrive as mailbox messages; replies go out through a Wasm-free *writer* process that owns the socket sink, so the guest never touches the raw socket.
3. **SSE serving** — a per-connection `sse::serve` handler streaming over a bounded, back-pressured `wasi:http` body — it parks under back-pressure rather than busy-spinning, and exits cleanly on client disconnect.
4. **Declarative routing** — a per-listener `[serve.routes]` table maps `"METHOD /path/:param" = "component#action"` (`:name` params, trailing `*` wildcard, specificity literal > param > wildcard). Handlers are **named actions** — no forced `main`; a Rust handler is `#[rusm_rs::handlers] pub mod api { pub fn home(req, params) -> Response { … } }`.
5. **`rusm serve`** — hosts every `rusm.toml [[serve]]` entry (`protocol` = `http`|`sse`|`ws`, `listen`) on a real port, and `rusm new <name>` scaffolds a ready-to-serve app, so `rusm new hello && cd hello && rusm build && rusm serve` then `curl` works end-to-end.
6. **All three guest languages serve all three protocols** — Rust and Go compile to `wasi:http`/the actor world; TypeScript runs on embedded rquickjs runners: `http_server_js` and the raw-`wasi:http` **js-http-runner** (`export default { fetch }`, pull-based streaming for SSE) plus a per-connection `ws_server_js` worker.
7. **The standard-WASI surface closed** — stock **`wasi:cli/run`** command components run unchanged (`WasmRuntime::spawn_command`); the TS runner gained a capability-gated streaming **outbound `fetch`** (over `wasi:http`, gated on the network capability) and **`crypto`** (getRandomValues/randomUUID over `wasi:random`) — enough to host real npm clients.

## Design highlights

- **Serving is always process-per-unit-of-work — there is no resident serving mode.** HTTP/SSE are process-per-request; WS is process-per-connection. Head-of-line blocking is impossible by construction, and a crash drops one unit, never the server. Shared state lives in a resident `[components.<name>]` service or durable `kv`, never in the ephemeral serving instance.
- **Wizer-preinitialized JS runners.** The QuickJS engine + the full host bridge are booted once at build time and snapshotted into the image, so each per-request instance copy-on-write starts *warm* and only evals the bundle and runs `fetch` — about **8× the cold per-request rate**, while still being instance-per-request and never resident.
- **The Wasm-free core stays Wasm-free.** All of the serving stack — hyper, `tokio-tungstenite`, `wasi:http` — lives only in `rusm-wasm`; `rusm-otp` never learns that HTTP exists. The WS writer process is a Wasm-free `rusm-otp` process owning the sink, so the sandboxed guest is never trusted with the socket.
- **Routing is config, not code.** `rusm-node::RouteTable` compiles the `[serve.routes]` table and bridges it into the routing-agnostic `rusm-wasm` `RoutedHttpServer`; the guest is pure handler code with no router and no `wit/` dir.
- **Benchmarked honestly, out-of-process.** The fair headline numbers come from `rusm-loadtest` running in a *separate* process against a live `rusm serve` port across a real socket — never sharing the server's CPU. The six co-resident dashboard demos are live and useful, but the credible numbers are the out-of-process ones.

## What this unlocks

A RUSM component is now a deployable web service. `rusm new && rusm build && rusm serve` is a working server in three commands, in any of the three languages. Because outbound `fetch` and `crypto` are real, a TypeScript guest can drive real npm clients (an LLM SDK, say) from inside the sandbox. The collaborative todo-board example — HTTP CRUD, a live SSE feed, WebSocket chat, and a resident store service — runs entirely on this surface.

And because **0.3.0** then matured the whole serving surface — a per-connection request **context** (route params → path-parameterised SSE), full WebSocket framing (text/close/ping-pong/subprotocols) with **permessage-deflate**, rich SSE events (`id`/`event`/`retry`) + Last-Event-ID resumption, per-listener resource & CSWSH controls (`max_connections`/`max_message_size`/`allowed_origins`), gzip/deflate **compression**, and native **TLS** (`https`/`wss`) — the serving story is production-shaped, not a demo.

## Try it

```sh
rusm new hello && cd hello && rusm build && rusm serve     # → curl http://127.0.0.1:8080/
cargo run -p rusm-loadtest -- http http://127.0.0.1:8080   # fair, out-of-process load test
cargo run -p rusm-loadtest -- ws   ws://127.0.0.1:8081
cargo run -p rusm-loadtest -- sse  http://127.0.0.1:8082
```

## Status

Functionally complete. Measured **out-of-process** over loopback (the fair headline): HTTP ~46k req/s at 0% errors; WS 256 held connections ~146k round-trips/s; SSE 256 held streams ~609k events/s; ~34k sandboxed-process-per-connection WS establishments/s (`conn` mode). The six co-resident dashboard tiles (`http-throughput`, `ws-echo`, `sse-fanout` and their `*-ts` twins) are live demos that share the node's CPU, so their figures differ by design from the fair headlines above.

The one **deferred** refinement is a native p3-typed `stream<u8>` WIT signature for the actor world — cosmetic standards-polish, since the handle-ABI byte streams are functionally complete and load-bearing for WS/SSE serving. `rusm-otp` stays Wasm-free throughout.

---

*Phase 12 (edge & cluster hardening) is planned: serve-path admission control (per-request timeout + request-body cap), default-bounded serve-path mailboxes, and signed cluster gossip ownership — network-edge and peer-trust gaps, not sandbox breaks. See the [roadmap](/about/roadmap).*
