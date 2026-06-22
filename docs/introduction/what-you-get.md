# What you get

**RUSM is Erlang/OTP's concurrency and fault-tolerance model, rebuilt in Rust, where
every process can be a sandboxed WebAssembly component** — "let it crash" meets "run
untrusted code safely," at millions of lightweight processes, with none of the
wasmCloud ceremony. The whole design rests on one principle: the actor core
(`rusm-otp`) is **Wasm-free**; WebAssembly is a pluggable process backend. That's why
it's both a great native actor runtime *and* a great Wasm host.

This page is the map. Each item links to the [concept](/deep-dive/the-process-model)
that teaches it — skim for what you need, then follow the arrow.

## Concurrency & the actor core

The Erlang core, rebuilt in Rust and Tokio — processes, mailboxes, supervision, fairness:


- **Massive lightweight concurrency (Tokio-driven)** — hundreds of thousands of
  processes over a few OS threads; spawning is near-free (~2.4M/sec measured). →
  [the process model](/deep-dive/the-process-model)
- **Isolated, lightweight processes** — one process = one task (and, for Wasm, one
  isolated instance); no shared mutable state.
- **Superior messaging** — per-process mailboxes, by-value messages, selective
  receive, ~21M msgs/sec. → [message passing](/deep-dive/message-passing)
- **Fault tolerance & supervision** — links, monitors, `trap_exit`, `spawn_link`,
  `exit`, one-for-one / one-for-all / rest-for-one, windowed restart-intensity. →
  [links & supervision](/deep-dive/links-and-supervision)
- **Preemptive fairness** — a runaway guest can't starve others; Wasmtime epoch
  interruption forces it to yield. → [epoch preemption](/deep-dive/epoch-preemption)
- **"Write blocking code, get async"** — `receive` parks the fiber and frees the
  worker. → [fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)
- **Process management** — named registry, timers, graceful shutdown, full
  introspection. → [process management](/deep-dive/process-management)
- **Backpressure & overload** — bounded byte-stream channels; opt-in bounded
  mailboxes that shed *user* messages but never system/exit signals.

## WebAssembly & safety

What makes a process a *sandboxed* process — the component model, default-deny capabilities,
and trap isolation:

- **WASM components (WASI p2 + p3)** — the modern component model, not just core
  modules (the headline difference from Lunatic). → [components & the actor
  world](/deep-dive/components-and-the-actor-world)
- **WASM core modules (wasip1)** — also runs raw preview-1 modules via a bridge.
- **Default-deny capability sandboxing** — `sandboxed` / `network-client` / `trusted`
  profiles + per-spawn overrides; per-instance memory caps; spawn-from-guest never
  escalates. → [permissions & sandboxing](/deep-dive/permissions-and-sandboxing)
- **Trap isolation** — a guest trap becomes that one process `Crashed`; the runtime
  and its neighbours are untouched.
- **Durable key-value storage** — an embedded, transactional store (`rusm-kv`/redb,
  no daemon) behind the default-deny `allow-storage` capability; the `kv` API is the same
  in Rust and TS. → [permissions & sandboxing](/deep-dive/permissions-and-sandboxing)
- **Guests in Rust, TypeScript, *or* Go** — the `#[service]` macro, the concealed typed
  client, the **shared rquickjs runner** (tiny TS components vs jco baking an engine
  into every one) + bytecode precompile, plus native `fetch` and **`crypto.subtle`**
  (SHA/HMAC/AES-GCM) for TS; Go compiles via TinyGo to `wasm32-wasip2`, same actor world
  and wire. → [guests: Rust, TypeScript & Go](/deep-dive/guests)

## Serving & streaming

Run a component as a real web server — one process per request or per connection, never a
shared instance:

- **HTTP / WebSocket / SSE serving** — from a Rust, TypeScript, or Go component, always
  **process-per-unit-of-work**: a fresh sandboxed instance per HTTP/SSE request, one
  sandboxed process per WS connection. No head-of-line blocking, crash containment, and
  full isolation by construction; cheap on the pooled spawn path (~440k spawns/sec). →
  [the serving model](/deep-dive/the-serving-model)
- **Declarative routing** — a per-listener `rusm.toml` `[serve.routes]` subtable (one
  per `[[serve]]` HTTP/SSE listener, so multiple ports route independently) maps
  `"METHOD /path/:param" = "component#action"` (`:name` path param, trailing `*`
  wildcard); the host gateway resolves it (most-specific wins; 405 on method mismatch,
  404 on no match). Rust handlers are `pub fn`s under `#[rusm_rs::handlers]` — no `main`,
  no router code.
- **Streaming & async** — SSE and WS as per-connection handler processes
  (`open`/`message`/`close`, one process per connection, streaming for its life), Tokio
  back-pressure throughout.
- **Shared state lives elsewhere** — never in the ephemeral serving instance: a
  long-lived `[components.<name>]` service (`resident = true`) reached over the actor API
  (`whereis` / `call` / `send`), or durable `kv`. Pairs with the `pubsub::Topics` primitive (keyed fan-out,
  monitor-based pruning): one publish → every connected client, no subscriber
  bookkeeping in app code.
- **Cross-process byte streams** — a bounded, back-pressured byte channel between
  processes. → [byte streams](/deep-dive/byte-streams)

## Apps, clusters & DX

One CLI from scaffold to live node, plus cross-node clustering and live attach:

- **App model** — `rusm.toml` describes components and servers; `rusm build` (cargo
  wasm32-wasip2 / Bun, no jco) → `./wasm/`. → [the app model](/deep-dive/the-app-model)
- **CLI** — `rusm new` (scaffold), `rusm run`, `rusm serve`, `rusm dev` (watch +
  reload), `rusm attach` (a live REPL into a local or remote node).
- **Dynamic JS** — load a guest's JS from a URL or the durable `kv` store (`source = "…"`)
  to **deploy live with no node rebuild**, or run code chosen *at runtime* — generated,
  fetched, or user-submitted — inside an operator-defined sandbox (`dynamic = "js"` +
  `spawn-from`): the guest picks the code, never the capabilities. → [Dynamic JS](/build-an-app/dynamic-js)
- **Distributed clustering** — `ClusterNode::connect` (the `Node.connect` equivalent),
  cross-node send, a gossiped global registry, remote spawn, all over QUIC + **mutual
  TLS**. → [distributed nodes](/deep-dive/distributed-nodes)
- **Live attach** — inspect/control a running node's processes live. →
  [live attach](/deep-dive/live-attach)
- **DX: infra never bothers you** — you write *application functions*; RUSM owns
  *all* the infrastructure (spawn/receive/reply/supervise/sockets).
- **No funky rules** — no execution-time cap, no "this must be a service," no
  lattice/provider ceremony.

## Observability & quality

You can see what the runtime is doing, and the numbers are honest:

- **Live process statistics** — an Erlang-`observer`-style view (process count,
  scheduler load, memory, per-instance table), nearly free.
- **Live benchmark dashboard** — real scenarios streamed to a React/uPlot UI. →
  [benchmark & dashboard](/about/benchmark-dashboard-and-observer)
- **Fair, out-of-process benchmarking** — `rusm-loadtest` drives a real `rusm serve`
  port from a separate process, so the numbers are the server's.
- **TDD, ~100% coverage; OTP first, WASM second** — the dependency graph enforces the
  Wasm-free core. → [architecture](/about/architecture)

## Where it's going

The core is audited-solid today; 0.3.0 matured the serving surface (connection context,
WebSocket framing + permessage-deflate, rich SSE events, per-listener resource/CSWSH
controls, compression, and native **TLS**). **Phase 12** (the serve-path admission-control
remainder — request-body cap + per-request timeout → `503` — default-bounded serve
mailboxes, and cluster gossip authentication) is explicitly planned. See the
[roadmap](/about/roadmap).
