# Changelog

All notable changes to RUSM are documented here. The project is a Cargo workspace of
several crates plus the `rusm-ts` npm package; **as of 0.2.0 they version in lock-step**
(earlier releases versioned independently, so each pre-0.2.0 entry lists the versions it
shipped). Format follows [Keep a Changelog](https://keepachangelog.com/); the project is
pre-1.0, so minor/patch numbers don't yet imply SemVer guarantees.

## [0.6.0] — 2026-06-24

Deadline-aware calls, two new dynamic-component flavours, and guest timer access — all three
guest SDKs advance in lock-step.

### Added
- **`call_timeout` / `CallTimeout` / `callTimeout`** — deadline-bounded cross-process call
  across all three guest SDKs (RS/Go/TS), returning `Err("timeout")` / throws `Error("timeout")`
  when the deadline expires. The timeout spans the entire call, including any set-aside messages.
  TS additionally exposes `withTimeout(ms)` on the typed client proxy, applying a per-call
  deadline to every subsequent call on that proxy. Documented in *Call another component —
  Call with a deadline*.
- **`dynamic = "wasi-cli"` template** — run any stock `wasm32-wasip2` CLI component (no RUSM
  actor world required) as a sandboxed one-shot process via `spawn-from`. Uses the same
  content-addressed compile cache, capability profile, and `kv:`/`url:`/`inline:` source
  resolution as `dynamic = "wasm"`. Documented in *Dynamic WASM — Stock CLI tools*.
- **`entry =` config field** — override the entry-point export name for static and
  `dynamic = "wasm"` components (default `"run"`). Enables components built against a
  non-standard WIT world that exports a differently-named function. Documented in *Dynamic WASM
  — Custom entry export*.
- **`send_after` / `cancel_timer` in Wasm guests** — guest components can now schedule delayed
  messages and cancel them through the `rusm:runtime` WIT world. The three SDKs already exposed
  `sendAfter`/`cancelTimer` (TS), `send_after`/`cancel_timer` (RS), and `SendAfter`/`CancelTimer`
  (Go); this release wires the ops into the component host bridge so they reach the runtime.
  Documented in *Timers*.

## [0.5.0] — 2026-06-24

The bridge authoring model is now three-way: **Rust**, **TypeScript**, and **Go** bridge
hosts are all first-class. A bridge's host language is now a per-app choice, not a platform
constraint — Rust for zero-overhead, TS or Go for familiar toolchains and external-API bridges.

### Added
- **TypeScript bridge host (`host.ts`)** — write a bridge as a plain TS file; `rusm build`
  generates the Rust delegation shim and TS runner (a resident actor). Each call crosses the
  actor wire (~1–10 µs). No Rust required.
- **Go bridge host (`host.go`)** — full parity with `host.ts`; TinyGo compiles
  `bridges/<name>/` to `wasm/bridge-<name>.wasm`. WIT record params arrive as
  `json.RawMessage` for idiomatic Go unmarshaling.
- **`--template mailer`** — scaffold a complete transactional email bridge (Resend API) in
  all three bridge-host languages + all three guest languages.
  `rusm new <name> --template mailer`.
- **Three-flavour example groups** — `examples/weather-api/` and `examples/mailer/` each
  split into `rust/`, `typescript/`, and `go/` subdirectories, one standalone app per bridge
  host language. Each flavour has its own README, `.gitignore`, and `rusm.toml`.
- **Processes & messaging docs** — six new reference pages: *Spawn & lifecycle*, *Timers*,
  *Links & monitors*, *Supervision*, *Pub/sub with tags*, *Powered by Tokio*.
- **Serving docs revised** — HTTP, SSE, and WebSocket pages rewritten with concrete
  multi-component examples, `[serve.routes]`-first framing, and the bidirectional WS model.
- **Bridge authoring docs** — `add-your-own-functions.md` covers all three host languages
  with a host-language comparison table and `how-it-builds` detail for each.
- **Example READMEs** — comprehensive audit: missing READMEs added (`todo-board/`,
  `url-shortener/{typescript,rust,go}/`), all flavour READMEs verified for accuracy,
  consistency enforced across families.

### Fixed
- **`host.ts` delegation protocol** — three correctness fixes: envelope framing,
  return-value serialisation, and actor lifecycle on bridge-host shutdown.
- **Stale path references** — `examples/custom-bridge/` refs replaced by
  `examples/weather-api/`; old top-level `examples/{typescript,rust,go}/` refs replaced
  by `examples/todo-board/<lang>/`.
- **Mailer Go module path** — template and scaffold always emit `api/internal/…` (Go module
  is always `module api`), not the app-name-prefixed path.
- **`SDK_VERSION` in scaffold** — `rusm new` now generates `rusm-ts@^0.5.0`,
  `rusm-rs = "0.5.0"`, `rusm-go v0.5.0` instead of the stale `0.4.0` that was pinned.
- **`url-shortener/go` dep** — `go.mod` was on `rusm-go v0.4.0`; updated to `v0.5.0`.

## [0.4.2] — 2026-06-23

Closes two guest-SDK gaps that forced apps to hand-roll platform plumbing (raw wire / raw
`wasi:http`) — so application code stays application code.

### Added
- **`rusm-ts` `connect(name | pid)`** — a typed client over an **already-running** service (a
  resident), the TS twin of Rust's `Client::connect(pid)` / Go's `Call(pid, …)`. `spawn<T>` only
  ever started a *fresh* instance, so reaching a resident from TS previously meant hand-building
  the RPC envelope; `connect` makes it `await connect<Counter>("counter").bump(1)`.
- **`rusm-rs` `http::fetch`** — outbound HTTP for a Rust guest over raw `wasi:http`
  (capability-gated on the network capability), reusing the shared `Request`/`Response` (+ new
  `Request::get`/`post`/`.header` builders). A guest writes `http::fetch(&Request::post(url,
  body).header("authorization", token))` instead of hand-rolling the wasi:http plumbing — the
  proven fiber-parking path, not `wstd`.

## [0.4.1] — 2026-06-23

A **correctness** release for the guest RPC wire, surfaced by porting a real app
(genius-rusm) onto RUSM: the first GenServer-that-also-calls — a service that makes a `call`
while serving requests — exposed a latent flaw shared by all three guest SDKs. No ABI/WIT
change: a guest built against 0.4.1 runs unchanged on a 0.4.0 host.

### Fixed
- **Wire reply-matching `ref` collision (Rust, Go, TS).** A blocking `call` matched its reply
  by correlation `ref` alone. But `ref` is a per-process counter, so a **concurrent inbound
  request** from another process could carry the same `ref` and be mis-read as the reply (no
  `ok`/`err` → decoded to null/garbage → silently resolved the call wrong). Replies are now
  matched by **`ref` *and* shape** (must carry `ok`/`err`). Only affected a process that both
  serves requests and makes calls — the core Erlang/OTP GenServer pattern.
- **Selective-receive re-read loop (Rust, Go, TS).** While awaiting a reply, `call` set
  non-matching mail aside into the *same inbox `receive` drains first* — so it was re-read
  forever and the real reply (behind it in the mailbox) was never reached (a hang). Set-aside
  mail is now held in a call-local buffer and restored to the inbox front after the call (a
  proper selective receive), via a new single-sourced `unstash-front` actor primitive.
- **WS/SSE connection loops (Rust, Go, TS).** A monitor `__down` for a pid *other* than the
  connection's writer (e.g. a backend the handler itself monitors) was delivered to the
  handler as if it were an inbound frame/event. Stray `__down`s are now skipped; only the
  writer's death ends the connection.

### Notes
- A `#[service]` / `Service` dispatch loop is request/reply only — it skips non-request mail
  (including `__down`); react to a monitored process's death via a `Supervisor` or a
  hand-rolled receive loop.
- Cross-process stream `accept` is unambiguous by construction: a guest is single-fiber, so a
  process can have at most one `stream-accept` outstanding; concurrent streaming serializes.
- Coverage: Rust host unit tests for the reply classifier + the selective-receive restore;
  **Go & TS integration tests** drive real guests through the `ref`-collision against a shared
  noisy echoer; and **RS/Go/TS integration tests** assert the WS/SSE connection loop skips a
  stray `__down` (real echo guests, each connection-loop implementation).

## [0.4.0] — 2026-06-22

A **custom-bridges + dynamic-WASM** release. An app can define its own **native host
functions** — typed WIT functions backed by host Rust — and call them from **any** guest
(Rust, Go, **and** TypeScript) as ordinary imports: RUSM's compiled-in answer to a wasmCloud
capability provider (no lattice, no broker, no RPC, default-deny and gated by name).
Alongside it, **dynamic WASM** — a guest can now spawn a **compiled WASM component chosen at
runtime** (`spawn-from` a `kv:`/`url:`/`inline:` source, or a `dynamic = "wasm"` template),
compiled once and served from a content-addressed cache for hot re-spawns. Underneath, the
host's own built-in capabilities were refactored into the same single-source **`bridges/`**
layout. **One breaking change**: the `[[serve]]` `name` field is renamed to `component`. The
whole workspace + the `rusm-ts` package move to **0.4.0** in lock-step; `rusm-go` is tagged
`v0.4.0`.

### Added
- **Dynamic WASM** — `spawn-from(template, source)` now loads a **compiled WASM component**,
  not only a JS bundle: declare a runner template with `dynamic = "wasm"` (a capability
  profile, no fixed bundle) and a guest spawns instances from a runtime-chosen `kv:`/`url:`/
  `inline:` source. Backed by a **content-addressed compile cache** (keyed by the SHA-256 of
  the bytes, not the source string): the first spawn of a bundle compiles (cold, ~17 ms for a
  small component), every later spawn instantiates on the pooled fast path (hot, ~0.5 ms);
  single-flight so concurrent first-spawns compile once, and a freshness/idle TTL
  (`[node] dynamic_wasm_ttl_secs`, default 300 s) re-checks a source for new bytes and evicts
  idle artifacts. The chosen code always runs under the **template's** profile — the request
  picks *which* code, never *what it may do*. The same `source` mechanism also loads a remote
  `.wasm` for a `[components.<name>]`/`[[serve]]` (sniffed by the WASM magic), so a compiled
  component can deploy from a blob store, not only the local artifact.
- **`rusm kv`** — a CLI command to read/write the node's durable store from the shell
  (`set`/`get`/`list`/`rm`), chiefly to **publish a dynamic bundle** (`rusm kv set
  plugins/greeter wasm/greeter.wasm`) that a `kv:` source then loads. The node must be stopped
  (the store is single-writer).
- **`examples/dynamic-wasm`** — a runnable plugin host: an HTTP dispatcher spawns
  runtime-chosen compiled plugins (`greeter`, `shout`) in one sandbox, compiled once then hot.
- **Two live dashboard scenarios** — `custom-bridge` (a sandboxed guest calling an
  app-registered native bridge in a loop — ~310k round-trips/s, p50 ~12µs) and
  `dynamic-wasm` (runtime-loaded compiled components through the cache — ~110k hot spawns/s
  after the one-time cold compile), each a real engine with a `WasmRuntime::prepare_dynamic`
  embedding entry point for the latter.
- **Custom bridges** — an app declares `bridges/<name>/{bridge.wit, host.rs}` (its own WIT
  package + a native `impl <iface>::Host for BridgeHost`); `rusm build` generates the host
  glue, vendors the contract into each granted guest, and compiles a small host binary that
  registers it. Reachable only by a component whose profile lists `bridges = ["<name>"]`
  (default-deny). Carries the **full WIT value-type set** — records, variants, enums, lists,
  options, results, tuples — identical for every guest.
- **Bridges from every guest** — Rust (`#[rusm_rs::handlers(bridge = "…")]` →
  `crate::<iface>`), Go (`wit-bindgen-go` bindings), and **TypeScript** (a per-app js-runner
  rebuilt with the bridge compiled in + a generated `bridges.d.ts`). TS records/enums marshal
  JS↔Rust via `serde_json` inside the QuickJS runner; the host call itself stays a typed WIT
  call.
- **Scaffold a custom-bridge app** — `rusm new <name> --template weather` (the discoverable
  named template, alongside `--template todo-board`) or `rusm new <name> --bridges` scaffolds a
  complete `weather` bridge app, in **any guest language** — TypeScript, Rust, or Go (the TS
  guest path is now supported; the per-app js-runner is rebuilt with the bridge compiled in).
- **Docs & examples** — a task-oriented *Build an app* guide rebuilt around a runnable
  **URL-shortener** example (TypeScript/Rust/Go), an *Add your own functions* custom-bridges
  page, and the `examples/weather-api/rust` weather app.

### Changed
- **Internal `bridges/` single-source layout** — the host's built-in capabilities (`kv`,
  `log`, `streams`, `pg`, `serve`, `actor`) were each migrated to their own WIT interface
  under `bridges/<name>/`, single-sourced across host and guests. No guest-facing change;
  the actor ABI is unchanged.

### Breaking
- **`[[serve]]` `name` → `component`** — a routes-less HTTP/WS/SSE listener now names its
  single handler with `component = "…"` (was `name = "…"`). Update `rusm.toml`; unknown
  fields are a hard config error, so a stale `name =` fails fast with a clear message.

### Compatibility
- Custom bridges and the `bridges/` refactor are **additive** — existing guests and the actor
  ABI are unchanged. The only breaking change is the `[[serve]]` field rename above.

## [0.3.0] — 2026-06-19

A **serving-maturity** release: the HTTP / SSE / WebSocket surface gains a full,
production-grade feature set — a per-connection request context, mature WebSocket framing
with permessage-deflate, rich SSE events with resumption, per-listener resource & security
controls, response compression, and native TLS. **Everything is additive**: a 0.2.0 guest
and a 0.2.0 `rusm.toml` keep working unchanged — new config fields default off, new ABI ops
are opt-in, and the per-connection handshake is unchanged. The whole workspace + the
`rusm-ts` package move to **0.3.0** in lock-step; `rusm-go` is tagged `v0.3.0`.

### Added
- **Connection context** — per-connection WebSocket/SSE handlers read their request method,
  path, **route params**, query, headers, remote address, and negotiated subprotocol via a
  new `connection` ABI op (`.info()` on the handler's connection/stream). Combined with
  `[serve.routes]` on `ws`/`sse` listeners, this restores **path-parameterised streaming**
  (`/events/:plan/:collection/:id`). RS / TS / Go.
- **WebSocket frame maturity** — outbound **text** and **close** (code + reason) alongside
  binary, idle keep-alive **ping** + inbound-ping **pong**, **subprotocol negotiation**, and
  bounded back-pressure on the control path. RS `send_text`/`close`, TS `sendText`/`close`,
  Go `SendText`/`Close`.
- **SSE wire maturity** — **rich events** with `id` / `event` / `retry` framing
  (`Stream::emit` / `emit` / `Emit`) and **Last-Event-ID resumption** (event `id` + the
  `last-event-id` request header).
- **Resource & security controls** — per-`[[serve]]` `max_connections` (HTTP/SSE/WS),
  `max_message_size` (WS), and `allowed_origins` (WebSocket CSWSH protection).
- **Compression** — opt-in per-`[[serve]]` `compression`: **gzip** for routed HTTP handler
  responses and the SSE event stream (flushed per event), **permessage-deflate** (RFC 7692)
  for WebSocket.
- **Native TLS** — per-`[[serve]]` `[serve.tls]` cert/key serves the listener over `https`
  (HTTP/SSE) or `wss` (WebSocket); rustls + ring, terminated before hyper.

### Changed
- The WebSocket protocol now runs through an internal frame transport (`bridges/ws_codec`)
  built on tungstenite's frame primitives, so **permessage-deflate** (which no async Rust
  WebSocket library exposes) is available; no user-facing change, and the host-side echo
  baseline still uses tungstenite directly.

### Compatibility
- Fully backward-compatible with 0.2.0 (hence a minor bump, not a major): existing guests
  and manifests are untouched. Bumped to **0.3.0** because these are substantial new
  **features** — SemVer reserves the patch number for bug fixes.

## [0.2.0] — 2026-06-19

A guest-language and serving release: **Go joins Rust and TypeScript as a first-class
guest**, the **serving surface gains a full lifecycle** (WebSocket `close`, a reworked
per-connection SSE, per-listener response headers), and a complete **example app** ships
in all three languages. The whole workspace + the `rusm-ts` package move to **0.2.0** in
lock-step for this release.

### Added
- **Go guests (`rusm-go`)** — a first-class TinyGo → `wasm32-wasip2` guest SDK, so a RUSM
  process body can be Rust, TypeScript, **or Go**, interoperating over one JSON wire:
  `Pid`/`Send`/`Receive`/`Spawn`, the registry + process-group tags, byte streams, a
  `Service` + typed client (call/cast/stream/callback), an in-guest `Supervisor`, and a
  `web` HTTP/SSE/WebSocket serving API. `rusm new --lang go`; `rusm build` drives TinyGo.
- **Dynamic JS spawn-from** — `spawn_from` / `SpawnFrom` (RS/TS/Go): spawn a JS guest from
  an `inline:` string, a `kv:<bucket>/<key>` entry, or a `url:`/`http(s)://` source.
- **WebSocket `close` lifecycle hook** — handlers now see `open` / `message` / `close`
  (close fired on disconnect) across Rust, TypeScript, and Go.
- **Per-connection SSE** — SSE is now an `open`/`message`/`close` handler (one sandboxed
  process per connection, the SSE twin of WebSocket): RS `sse::serve`, TS
  `sse({ open, message, close })`, Go `web.Sse{ … }.Serve()`.
- **`[serve.headers]`** — per-listener response headers in `rusm.toml` (e.g. CORS so a
  browser can read a cross-origin SSE feed). Application policy, applied by the platform.
- **TS HTTP handlers gained `kv` + publish + `console`** on the js-http-runner — a
  `wasi:http` TypeScript handler can persist to `kv` and push to subscribers.
- **Example apps** — a runnable **collaborative todo board** in each language
  (`examples/{typescript,rust,go}`): five components — HTTP CRUD `api`, SSE `feed`,
  WebSocket `chat`, a resident `store` service, and a `reporter` worker — wired by
  process-group tags, with a polished web UI.
- **CLI** — `rusm new --template todo-board` scaffolds the full example app; `rusm
  --version` / `-V`; a richer top-level help and per-command `--help` (descriptions +
  examples).

### Changed
- **Examples reorganised** by audience: the three apps at `examples/{typescript,rust,go}`,
  performance benchmarks under `examples/benchmarks/`, and library/host-API examples under
  `examples/embedding/`. Removed the superseded `ts-app` and the internal harness demos
  (`headless_run`, `synthetic_source`, `observer_overhead`).
- **Docs** — the serving + guest guides point at the runnable example components; SSE is
  documented throughout as a per-connection handler (the old routed 3-arg SSE action is
  gone).

### Fixed
- **SSE disconnects no longer leak** — a dropped or refreshed SSE connection now reaps its
  handler process (releasing its process-group tag) the instant the connection ends,
  rather than lingering until the next keep-alive ping.
- **The census re-emits on tag release** — an *unlabeled* per-connection process (chat /
  feed) leaving a process-group tag now updates the census; previously only labeled exits
  re-triggered it, so chat/feed disconnects went unreported.
- **The `rusm new --protocol sse` scaffold** generated the removed routed-action form
  (didn't compile); it now scaffolds the per-connection `sse::serve` / `web.Sse` handler.
- **The example web page** — chat frames are decoded (no more `[object Blob]`) and the live
  feed shows a pulsing status + push counters.

### Versions
All workspace crates and the `rusm-ts` npm package ship at **0.2.0**; `rusm-go` is tagged
`v0.2.0`. (This release moves the previously independently-versioned crates to a single
lock-step version.)

## [0.1.5] — 2026-06-16

Observability for the process-group tags shipped in 0.1.4 — no API changes, runtime-only.

### Added
- **The lifecycle census reports process-group tag counts** — alongside the per-component
  line it now shows per-tag membership: `rusm census  pages-agent=4  plan:bfb8b1ed=4`,
  with tag names in green. So a node at `Info`+ shows how many live processes hold each
  tag (e.g. one `plan:<id>` group per in-flight unit of work).
- **Kill logging** — a `kill(pid)` logs `rusm kill  #<pid>`; a `kill_tag(tag)` logs one
  summary `rusm kill  <tag> → <n>` (the cause line ahead of each member's `exit`).

### Versions
rusm-otp 0.1.3 · rusm-logfmt 0.1.1 · rusm-cli 0.1.5. (rusm-wasm/node/cluster/kv/wire and
the guest SDKs are unchanged — their caret deps resolve the new otp/logfmt, so they are
not republished.)

## [0.1.4] — 2026-06-16

### Added
- **Process groups (Erlang `pg`)** — the scoped-cancellation primitive. A process tags
  *itself* with `register_tag(tag)` / `unregister_tag(tag)`; `whereis_tag(tag)` lists live
  members; `kill_tag(tag)` terminates the whole group. One tag → many pids, one pid → many
  tags; memberships are reaped on exit by the same reaper that releases names, and tags add
  **zero hot-path cost**. `kill_tag` is gated by the `process-control` capability (like
  `kill`); self-tagging is unprivileged. Available from both guests — `register_tag` /
  `kill_tag` / `whereis_tag` in **rusm-rs**, `Process.registerTag` / `killTag` /
  `whereisTag` in **rusm-ts** — backed by the Wasm-free `rusm-otp` core.
- **`rusm new --lang generic`** — scaffold a bring-your-own-component app; `rusm build`
  copies a pre-built wasip2 `.wasm` into `wasm/` (prefers `<name>.wasm`, errors on
  ambiguity). (#3, thanks @thomas9911)

### Changed
- **pico-args** is now the single argument parser for the `rusm` CLI. (#2, thanks @thomas9911)
- Dropped the `xtask` crate; `make docs-deploy` now publishes the VitePress site to
  `gh-pages` directly — one fewer crate to build.

### Fixed
- The benchmark crate now builds on **Windows** (the unix-only `rlimit` soft-limit lookup
  is gated; Windows falls back to a 256-fd ceiling). (#1, thanks @thomas9911)

### Versions
rusm-otp 0.1.2 · rusm-wasm 0.1.4 · rusm-cli 0.1.4 · rusm-rs 0.1.2 · rusm-rs-macros 0.1.2 ·
rusm-ts 0.1.2. rusm-logfmt / rusm-kv unchanged at 0.1.0.

## [0.1.3] — 2026-06-15

A large release: durable storage, platform logging, declarative routing, and a warm-start
TS runner. First crates.io publish of **rusm-kv** and **rusm-logfmt**.

### Added
- **Durable key-value storage** for guests — the Wasm-free `rusm-kv` crate (embedded redb)
  behind the `storage` capability and a `kv-*` ABI.
- **`receive-timeout`** for guests (Erlang's `receive … after`).
- **pub/sub fan-out** as a primitive (`rusm-rs` `pubsub::Topics`) and **turnkey offloaded
  SSE** live fan-out, proven end-to-end.
- **Platform logging** — guests log via `console.*` (TS) / the `log` crate (Rust) to the
  host log, with a serving **access log**; gated by `[log] level`, no stdio wiring in guest
  code. Shared column widths + timestamps via `rusm-logfmt`.
- **Routed, per-request serving** — `#[rusm_rs::handlers]` named actions, a per-request
  HTTP/SSE gateway over the actor world, and SSE streaming handlers in the unified model.
- **`crypto.subtle`** for TS guests (RustCrypto), `btoa`/`atob`, capability-granted
  `process.env`, and dynamic bundle sourcing (load a component's JS from a URL or `kv`).
- `rusm serve` also brings up `[[components]]` (one node, one command).

### Changed
- **Breaking:** a registered component runs under its **own declared capability profile**
  (not the spawner's).
- **Breaking:** `[components.<name>]` is now a keyed map with a `resident` flag
  (boot-spawned + supervised), replacing the dead `restart` key.
- **Breaking:** routes are scoped to their listener — `[serve.routes]` (was a global
  `[routes]`).
- **Breaking:** disabled the component-model-async ABI to stop a `wasi:io` busy-poll.

### Performance
- **wizer pre-initialization** of the js-runner and js-http-runner — every TS guest
  CoW-starts *warm* (engine + bridge booted once at build time); ~8× TS HTTP throughput,
  still instance-per-request. Closed-loop benchmark load made steady at all speeds.

### Fixed
- Outbound `fetch` parks instead of busy-polling (kills an 800% CPU / server-stall).
- `js-runner` dispatched the request before writing the body (POST bodies were lost).
- `rusm-logfmt` inherits license/repository from the workspace (publish-blocking metadata).

### Versions
rusm-wasm 0.1.3 · rusm-cli 0.1.3 · rusm-otp 0.1.1 · rusm-node 0.1.1 · rusm-rs 0.1.1 ·
rusm-rs-macros 0.1.1 · rusm-ts 0.1.1 · **rusm-kv 0.1.0 (new)** · **rusm-logfmt 0.1.0 (new)**.

## [0.1.2] — 2026-06-10

### Fixed
- A string `Response` now defaults to `Content-Type: text/plain;charset=UTF-8`.

### Versions
rusm-cli 0.1.2 · rusm-wasm 0.1.2.

## [0.1.1] — 2026-06-10

### Fixed
- `rusm-wasm` UTF-8 hotfix.

### Versions
rusm-cli 0.1.1 · rusm-wasm hotfix.

## [0.1.0] — 2026-06-10

Initial crates.io release — the RUSM runtime, guest SDKs, and CLI, built through Phase 11.

### Added
- **`rusm-otp`** — the Wasm-free Erlang/OTP core: lightweight processes (one Tokio task
  each), message passing, links/monitors/exit signals, `trap_exit`, supervision with
  restart intensity, a sharded named registry, timers, graceful shutdown, TCP
  (process-per-connection), introspection, and back-pressured byte streams.
- **`rusm-wasm`** — the Wasmtime component host: instance-per-process guests over the
  component model (WASI p1/p2/p3), the `rusm:runtime` WIT actor world, default-deny
  capability profiles + a memory limiter, and an optimized spawn path (pooling allocator +
  CoW + per-module `InstancePre`).
- **`rusm-cluster`** — Wasm-free distributed transport over QUIC + mutual TLS (cross-node
  send, gossiped global registry, remote spawn, live attach).
- **Guest SDKs** — `rusm-rs` / `rusm-rs-macros` (Rust) and the embedded TS runner; plus
  `rusm-wire` and `rusm-node`.
- **`rusm-cli`** — the `rusm` binary: `new` / `build` / `run` / `dev` / `serve` /
  `node start` / `attach`.

### Versions
rusm-otp · rusm-wasm · rusm-node · rusm-cluster · rusm-rs · rusm-rs-macros · rusm-wire ·
rusm-cli. (rusm-kv and rusm-logfmt were first published in 0.1.3.)

[0.1.5]: https://github.com/archan937/rusm/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/archan937/rusm/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/archan937/rusm/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/archan937/rusm/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/archan937/rusm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/archan937/rusm/releases/tag/v0.1.0
