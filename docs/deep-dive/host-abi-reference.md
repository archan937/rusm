# Host ABI reference

A RUSM guest reaches the runtime through the **`rusm:runtime` actor ABI** — the
Erlang `Process` API (self, send, receive, registry, introspection, kill, …),
backed by thin calls into `rusm-otp`. The ABI comes in two equivalent shapes, one
per artifact kind, plus the standard WASI interfaces.

## Components — the `rusm:runtime` WIT actor world (wasip2/p3)

A WASI **component** imports the `rusm:runtime/actor` interface (a real WIT world,
bound with `wasmtime::component::bindgen!`), so a guest in any language calls typed
functions. Several capabilities are **sibling interfaces** — platform *bridges* split out
of the core: `rusm:runtime/{kv, log, pg, serve, streams}` (and the shared `types`); all are imported
into the one `process` world:

| Function | Meaning |
| --- | --- |
| `own-pid() -> pid` | the calling process's own pid |
| `send(to: pid, msg: list<u8>)` | enqueue bytes into another process's mailbox |
| `receive() -> list<u8>` | **async** — park the fiber until a message arrives |
| `receive-timeout(timeout-ms) -> option<list<u8>>` | like `receive`, but gives up after a deadline — Erlang's `receive … after` (heartbeats, deadlines) |
| `stash(msg) / unstash()` | set the just-received message aside (keeping its host-side metadata) while awaiting a different one, then return everything stashed to the front of the mailbox — the host primitive behind the SDKs' selective `call` (Erlang's selective `receive`); host-side so per-message metadata survives the deferral |
| `list-processes() -> list<pid>` | all live pids |
| `info(pid) -> option<process-info>` | links, monitors, names, label, mailbox depth, trap-exit |
| `is-alive(pid) -> bool` / `kill(pid) -> bool` | liveness / forced termination |
| `register(name) / whereis(name) / unregister(name)` | the named registry (1 name → 1 pid) |
| `set-label(label)` | a human-readable label for the observer |
| `send-after(to, delay-ms, msg) -> timer-id` / `cancel-timer(id) -> bool` | a delayed send (Erlang's `send_after`) and its cancellation |
| `spawn(name) / monitor(pid) / supervise(…)` | start, watch, and supervise child components (capability-gated) |
| `spawn-from(name, source) -> pid` | spawn a **dynamic** runner-template instance from a runtime source (`inline:` / `kv:` / `url:`) under the template's declared profile — see [dynamic JS](/build-an-app/dynamic-js) (capability-gated) |

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

### The `streams`, `pg`, and `log` interfaces (sibling bridges)

The same way `kv` split out, these capabilities are their own bridge interfaces — each owned
end-to-end in `bridges/<name>/`, materialized by `make sync-bridges`, imported into the one
`process` world. Same functions as before; only the interface they live in changed.

**`rusm:runtime/streams`** — cross-process byte streams (Tokio back-pressured); `*-write`/
`*-read`/`*-accept` suspend the fiber rather than busy-spin. Guest: `rusm_rs::Stream`, the TS
`Process.openStream`/`Stream`, Go's `OpenStream`.

| Function | Meaning |
| --- | --- |
| `stream-open(to: pid) -> option<stream-id>` | open a stream; the read end is delivered to `to` |
| `stream-write(handle, chunk) -> bool` / `stream-close(handle)` | write a chunk (back-pressured) / signal EOF |
| `stream-accept() -> stream-id` / `stream-read(handle) -> option<list<u8>>` | accept an incoming stream / read the next chunk (`none` = EOF) |

**`rusm:runtime/pg`** — process-group tags (Erlang's `pg`), RUSM's pub/sub primitive
(subscribe = `register-tag`, publish = `whereis-tag` + `send`). Guest: `rusm_rs::{register_tag, …}`,
the TS `Process.registerTag`/etc., Go's `RegisterTag`/etc.

| Function | Meaning |
| --- | --- |
| `register-tag(tag)` / `unregister-tag(tag)` | join / leave a tag (this process; unprivileged) |
| `whereis-tag(tag) -> list<pid>` | live members of a tag |
| `kill-tag(tag) -> u32` | terminate a whole group (count); gated by **process-control**, like `kill` |

**`rusm:runtime/log`** — platform logging (a polyfill bridge): a guest's standard logging —
`console.*` (TS), the `log` crate (Rust), `log`/`slog` (Go) — routes to `log(level, message)`;
the host stamps time, `component#pid`, and the severity colour, gated by the node `[log] level`.

**`rusm:runtime/serve`** — per-connection serving controls for a WebSocket/SSE handler the
host spawned for one accepted connection (a normal process gets `none`/`false`). Guest: the
ergonomic `ws::Connection`/`sse::Stream` (RS), `web.Conn`/`web.Stream` (Go), `Process`
methods (TS).

| Function | Meaning |
| --- | --- |
| `connection() -> option<connection-info>` | this handler's request context — method, path, captured **route params**, query, headers, remote address, negotiated subprotocol |
| `ws-send-text(payload) / ws-close(code, reason)` | a WebSocket handler's outbound **text** frame / **close** with status + reason (binary frames take the plain `send`→writer-process path) |
| `sse-send(data, event?, id?, retry?) -> bool` | an SSE handler's **rich event** (`id`/`event`/`retry`); a plain `data:` event takes the `send`→writer path |

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
store (the `kv` interface, backed by the Wasm-free `rusm-kv` crate over redb) — a
sandboxed process has none. See
[permissions & sandboxing](/deep-dive/permissions-and-sandboxing).

## Compatibility — standards-first, superpowers opt-in

RUSM is a **standard WASI host** (p1/p2/p3). A standard component or core module —
including one built with `cargo component` or [`wstd`](https://github.com/bytecodealliance/wstd)
(the Bytecode Alliance's guest-side async std) — **runs unchanged**, to the extent
it imports interfaces RUSM hosts. The `rusm:runtime` actor world is **purely
additive and opt-in**: import it for the Erlang `Process` API, or ignore it and
RUSM is just a fast, sandboxed WASI runtime. So there is no RUSM-specific
convention to adopt, and nothing to make code non-portable.

`wstd` itself is a *guest* library, not a host contract — "wstd compatibility"
simply means hosting the standard WASI interfaces a wstd guest imports. Both pieces
that make any standard component fully drop-in **shipped in Phase 11**:

- **Entrypoint:** alongside RUSM's bare exported `run` func, stock **command**
  components that export `wasi:cli/run` run unchanged (`WasmRuntime::spawn_command`,
  which shares the same store-build path as the actor entrypoint).
- **`wasi:http`:** RUSM hosts `wasi:http` (p2 + p3) — inbound for HTTP/SSE serving and
  a capability-gated, streaming **outbound `fetch`** — so a wstd HTTP guest just works.

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
