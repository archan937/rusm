# Host ABI reference

A RUSM guest reaches the runtime through the **`rusm:runtime` actor ABI** — the
Erlang `Process` API (self, send, receive, registry, introspection, kill, …),
backed by thin calls into `rusm-otp`. The ABI comes in two equivalent shapes, one
per artifact kind, plus the standard WASI interfaces.

## Components — the `rusm:runtime` WIT actor world (wasip2/p3)

A WASI **component** imports the `rusm:runtime/actor` interface (a real WIT world,
bound with `wasmtime::component::bindgen!`), so a guest in any language calls typed
functions. Several capabilities are **sibling interfaces** — platform *bridges* split out
of the core: `rusm:runtime/{kv, log, streams}` (and the shared `types`); all are imported
into the one `process` world:

| Function | Meaning |
| --- | --- |
| `own-pid() -> pid` | the calling process's own pid |
| `send(to: pid, msg: list<u8>)` | enqueue bytes into another process's mailbox |
| `receive() -> list<u8>` | **async** — park the fiber until a message arrives |
| `receive-timeout(timeout-ms) -> option<list<u8>>` | like `receive`, but gives up after a deadline — Erlang's `receive … after` (heartbeats, deadlines) |
| `list-processes() -> list<pid>` | all live pids |
| `info(pid) -> option<process-info>` | links, monitors, names, label, mailbox depth, trap-exit |
| `is-alive(pid) -> bool` / `kill(pid) -> bool` | liveness / forced termination |
| `register(name) / whereis(name) / unregister(name)` | the named registry (1 name → 1 pid) |
| `register-tag(tag) / unregister-tag(tag) / whereis-tag(tag) -> list<pid>` | process groups (Erlang `pg`: 1 tag → many pids); self-tag is unprivileged |
| `kill-tag(tag) -> u32` | terminate a whole group (returns the count); gated by **process-control**, like `kill` |
| `set-label(label)` | a human-readable label for the observer |
| `spawn(name) / monitor(pid) / supervise(…)` | start, watch, and supervise child components (capability-gated) |
| `connection() -> option<connection-info>` | a per-connection serving (WS/SSE) handler's request context — method, path, captured **route params**, query, headers, remote address, negotiated subprotocol |
| `ws-send-text(payload) / ws-close(code, reason)` | a WebSocket handler's outbound **text** frame / **close** with status + reason (binary frames take the plain `send`→writer-process path) |
| `sse-send(data, event?, id?, retry?) -> bool` | an SSE handler's **rich event** (`id`/`event`/`retry`); a plain `data:` event takes the `send`→writer path |

### The `kv` interface — durable storage (a platform bridge)

Durable key-value storage is a sibling interface, `rusm:runtime/kv`, imported into the same
`process` world (so a guest gets it alongside `actor`). It is authored as a **bridge** —
one capability, owned end-to-end in [`bridges/kv/`](https://github.com/archan937/rusm/tree/main/bridges/kv)
(`bridge.wit` + `host.rs` + `guest.{rs,go,js}`) and materialized into every crate by
`make sync-bridges`. Gated by the **storage** capability (default-deny).

| Function | Meaning |
| --- | --- |
| `kv.get(bucket, key) -> option<list<u8>>` | the stored value, or `none` |
| `kv.set(bucket, key, value)` | store (overwrite) |
| `kv.delete(bucket, key) -> bool` | remove; was-present |
| `kv.exists(bucket, key) -> bool` | membership |
| `kv.list(bucket) -> list<string>` | every key, sorted |

The ergonomic guest wrappers are unchanged — `rusm_rs::kv::bucket(..)`, the TS `kv.bucket(..)`
global, Go's `OpenBucket(..)` — each backed by the Wasm-free `rusm-kv` crate over redb.

Composition is **message passing** (spawn instances, then `send`/`receive`/
`register`/`whereis`) — *not* WIT inter-component wiring, and no lattice. Standard
**WASI p2 and p3** (`@0.2.0` and `@0.3.0` `wasi:cli`/`clocks`/`filesystem`/
`random`/`sockets`) are wired on the same component linker, gated by the process's
capability profile.

## Core modules — the raw `rusm::*` ABI (wasip1)

A `wasm32-wasip1` **core module** can't pass a WIT `list<u8>`, so the same
operations are flat imports under the `rusm` namespace that marshal through the
guest's exported linear `memory` (pointer + length):

| Import | Signature |
| --- | --- |
| `own_pid` / `notify` | `() -> i64` / `()` (the latter bumps the shared progress counter) |
| `send` | `(to: i64, ptr: i32, len: i32)` |
| `receive` | `(ptr: i32, cap: i32) -> i32` (async; returns the message length) |
| `list_processes` | `(ptr: i32, cap: i32) -> i32` (writes pids; returns the count) |
| `is_alive` / `kill` | `(pid: i64) -> i32` |
| `register` / `whereis` / `unregister` | `(ptr: i32, len: i32) -> i32`/`i64`/`i32` |
| `set_label` | `(ptr: i32, len: i32)` |
| `stream_open` | `(to: i64) -> i64` — open a byte stream to a process; returns a stream id |
| `stream_write` / `stream_close` | `(id, ptr, len) -> i32` (async, back-pressured) / `(id)` |
| `stream_accept` / `stream_read` | `() -> i64` (async) / `(id, ptr, cap) -> i32` (async; `-1` at EOF) |

Both shapes call the *same* `rusm-otp` operations; only the calling convention
differs. Standard **WASI preview1** (clocks, random, env, stdio, scoped fs) is
wired via `wasmtime_wasi::p1`, capability-gated.

## Capabilities (default-deny)

Every grant maps onto standard WASI plus a `StoreLimiter` memory cap. Named
profiles — `Sandboxed` (CPU + bounded heap only), `NetworkClient` (+ outbound
network), `Trusted` (+ stdio, large heap, durable **storage**) — set defaults; a
per-spawn `Capabilities` builder overrides them (`allow-spawn`, `allow-process-control`,
`allow-storage`, …). The **storage** grant opens the node's embedded durable key-value
store (the `kv-*` ABI, backed by the Wasm-free `rusm-kv` crate over redb) — a
sandboxed process has none. See
[permissions & sandboxing](./concepts/permissions-and-sandboxing.md).

## Compatibility — standards-first, superpowers opt-in

RUSM is a **standard WASI host** (p1/p2/p3). A standard component or core module —
including one built with `cargo component` or [`wstd`](https://github.com/bytecodealliance/wstd)
(the Bytecode Alliance's guest-side async std) — **runs unchanged**, to the extent
it imports interfaces RUSM hosts. The `rusm:runtime` actor world is **purely
additive and opt-in**: import it for the Erlang `Process` API, or ignore it and
RUSM is just a fast, sandboxed WASI runtime. So there is no RUSM-specific
convention to adopt, and nothing to make code non-portable.

`wstd` itself is a *guest* library, not a host contract — "wstd compatibility"
simply means hosting the standard WASI interfaces a wstd guest imports. Two items
are on the roadmap (Phase 11) to make any standard component fully drop-in:

- **Entrypoint:** RUSM currently invokes a bare exported `run` func; standard
  *command* components export the `wasi:cli/run` interface. Supporting that export
  as an entrypoint lets stock command components run as-is.
- **`wasi:http`:** wstd's HTTP layer imports `wasi:http`, which RUSM will host
  alongside the HTTP-serving work — then wstd HTTP guests just work.

## Wire protocol (node ↔ dashboard / REPL)

Defined in `rusm-bench` `protocol.rs`, mirrored in the dashboard's `types.ts`
(`serde` tagged, `snake_case`).

Server → client:

- `hello { scenarios: ScenarioMeta[], profiles: ResourceProfileMeta[] }` — the
  scenario and resource-profile menus, sent on connect.
- `tick { frame: Frame }` — one sampled frame per tick.
- `error { message: string }` — a rejected command.

Client → server:

- `run { scenario: string }`, `stop`
- `set_observer_detail { enabled: bool }`
- `set_resource_profile { profile: string }` (`light` / `balanced` / `max`)

A `Frame` = `{ scenario, running, uptime_ms, ops_per_sec, peak_concurrent,
latency, throughput, observer, profile }`. Each `ScenarioMeta` carries a `unit`
(`count` or `bytes`) so the dashboard formats throughput correctly.
