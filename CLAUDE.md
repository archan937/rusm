# CLAUDE.md — working notes for RUSM

RUSM is an **Erlang-inspired WebAssembly runtime in Rust**: isolated lightweight
processes (one Wasm instance = one Tokio task), message passing, supervision,
per-actor sandboxing, "write blocking code → runtime makes it async", and secure
distributed clusters you can hook into live. See `README.md` for the pitch and
`docs/` for the full story.

## Status

**Phases 0–11 functionally complete (of 12; the native `stream<u8>` WIT signature is the one deferred refinement — handle-ABI byte streams work); Phase 12 (hardening) planned.** RUSM **hosts real WASM components** as isolated,
supervised processes, **clusters across nodes**, and is **hardened for scale**:
an opt-in **on-demand instance tier** (`WasmRuntime::with_overflow` — spawn past the
pooled cap onto an on-demand engine, bounded by memory not a fixed size), **opt-in
bounded mailboxes** (`Runtime::with_mailbox_capacity` — load-shed *user* messages,
never system signals), **mutual-TLS cluster security** (`ClusterCa` per-node certs,
foreign-CA peers rejected), and **windowed supervisor restart-intensity** (both
guests) — all with no spawn/message regression (~440k component spawns/s, ~21M
msgs/s hold). **Phase 11 (serving & standard-WASI surface) is functionally complete**
(native `stream<u8>` signature deferred): a component runs as a
high-throughput **HTTP / WS / SSE** server, all in `rusm-wasm` (hyper +
`tokio-tungstenite` + `wasi:http`; `rusm-otp` stays Wasm-free). `HttpServer`
(instance-per-request `wasi:http` via `ProxyPre`, ~64.5k req/s lean), `WsServer` (one
sandboxed **component process per connection** — inbound frame → mailbox message,
replies via a Wasm-free writer process that owns the socket sink; ~192k echo
round-trips/s, sandbox cost inside noise), and **SSE** (a `wasi:http` streaming body).
**All three guest languages serve all three**:
RS and **Go** compile to `wasi:http`/the actor world (Go via TinyGo → `wasm32-wasip2`);
**TS** runs on embedded rquickjs runners —
`http_server_js` + the raw-`wasi:http` **js-http-runner** (runs `export default { fetch }`,
pull-based streaming for SSE; **wizer-pre-initialized** — the QuickJS engine + bridge are
booted once at build time and snapshotted into the image, so each per-request instance
CoW-starts warm and only evals the bundle + runs `fetch`: ~8× the cold per-request rate,
still instance-per-request/never-resident; built via `js-http-runner/build.sh`:
wit-bindgen core module → wizer → `wasm-tools component new`) and `ws_server_js` (a TS worker, one process per
connection). **The unified serving model: serving is ALWAYS process-per-request
(HTTP/SSE) / process-per-connection (WS) — there is no "resident" serving mode**
(removed; resident-vs-per-call now lives only in `[components.<name>]` via the
`resident` flag, and shared state in
a resident `[components.<name>]` service or `kv`, never in the ephemeral serving instance — so
head-of-line blocking is impossible by construction and a crash drops one unit).
**Routing is declarative** in a per-listener `rusm.toml` `[serve.routes]` subtable (one
per `[[serve]]` HTTP/SSE listener, so multiple ports route independently) —
`"METHOD /path/:param" = "component#action"` (`:name` params, trailing `*` wildcard;
`#` separates component from action since `:` is reserved for `kv:`/`url:`; specificity
literal > param > wildcard; matched-path-wrong-method → 405, no match → 404), compiled
by `rusm-node::RouteTable` and bridged into the routing-agnostic `rusm-wasm`
`RoutedHttpServer` (`bridges/routed.rs`: resolve → spawn the matched handler fresh on
the optimized spawn path → dispatch the action over the actor wire → buffered or
chunked-streamed reply). **Handlers are named functions** ("actions"), no forced
`main`: a Rust handler component is `#[rusm_rs::handlers] pub mod api { pub fn
home(req, params) -> Response { … } }`; **SSE is a per-connection
`rusm_rs::sse::serve` handler** (`open`/`message`/`close`, the SSE twin of WS — route-less,
loaded by name like a WS component), streaming over a **bounded, back-pressured** byte stream (parks under back-pressure —
never busy-spins — and exits on client disconnect, guarded by a routed
disconnect-teardown test). **`rusm serve` is live**: it hosts `rusm.toml [[serve]]`
entries (`component` for a routes-less listener, `protocol` = `http`|`sse`|`ws`, `listen`)
on real TCP ports — each listener's non-empty `[serve.routes]` subtable
routes each HTTP/SSE request to a `#[handlers]` component per request; WS runs one
component process per connection; TS HTTP/SSE keep the handler-less `wasi:http`
`export default { fetch }` path (no routes table needed). The node only serves; it never generates
load. **`rusm new <name>`** scaffolds an app (a zero-dependency TS HTTP component
`components/api/index.ts`, a `rusm.toml` `[[serve]]` entry, `.gitignore`, README) so
`rusm new hello && cd hello && rusm build && rusm serve` then `curl
http://127.0.0.1:8080/` works end-to-end. **Serving is benchmarked two ways.** The
**fair, credible headline numbers** come **out-of-process** from the
`bench/rusm-loadtest` (`rusm-loadtest`) binary against a real `rusm serve` port — the
load generator runs in a separate process, never sharing the server's CPU, and
crosses a real socket. Its modes: `http` uses **balter** (a Tokio-native load
framework) as a fixed-rate sweep (balter's auto-saturation control is too cautious in
the sub-ms loopback regime, so we drive its constant-rate controller and sweep
ourselves — every number measured, none extrapolated); `ws` & `sse` use a
tokio-native connection-capacity harness (held connections sustaining echo
round-trips / draining events); `conn` is a connection-establishment storm
(sandboxed-process-per-connection WS establishments). Measured out-of-process
(loopback): HTTP ~46k req/s at 0% errors; WS 256 held connections ~146k
round-trips/s; SSE 256 held streams ~609k events/s; `conn` ~34k
sandboxed-process-per-connection WS establishments/s. The **six serving dashboard
scenarios are co-resident live demos** (`http-throughput`, `ws-echo`, `sse-fanout`
and their `*-ts` twins): each spins up the same real in-process WASM server and
drives it through the shared `rusm-loadtest` path (a steady **closed-loop** driver for
HTTP — a fixed set of outstanding requests that holds the tile at the server's real
ceiling, never flooding or collapsing whatever the guest's speed — and a
connection-capacity harness for WS/SSE held connections; the fair out-of-process headline
still uses balter's rate sweep), with load generator and
server sharing the node process — so live tile figures (http-throughput ~20k req/s,
ws-echo ~195k rt/s, sse-fanout ~695k events/s) differ by design from the fair
out-of-process headlines above. The Wasm-free
**`rusm-cluster`** crate (over `rusm-otp`, never Wasmtime) connects nodes over
**QUIC + TLS** (quinn + rustls/ring; **mutual TLS** — a `ClusterCa` issues per-node
certs, or a shared self-signed `Identity`): a `ClusterNode`
wraps a `Runtime` with a QUIC endpoint, exchanges names on a per-peer **control
stream**, and routes each message on its own **uni-stream**. It gives cross-node
`send`, a **gossiped global registry** (`register_global`/`whereis_global`/
`send_global`), **remote spawn** (named `Spawnable` factories), and **live attach**
(`remote_pids`) over one request/reply control-plane RPC — ~550k cross-node msgs/s,
~39µs p50 round-trip (the standalone `cluster_fanout` bench). The live
`distributed-fanout` dashboard scenario now runs on this real engine — **all nineteen
dashboard scenarios are real; none remain synthetic** (the ten core engines:
spawn-storm, ping-pong, fault-recovery, connection-storm, connection-scale, fairness,
module-storm, component-storm, stream-pipe, distributed-fanout; the six
co-resident serving demos: `http-throughput`, `ws-echo`, `sse-fanout` and their `*-ts`
twins — the fair serving headline numbers still come from `rusm-loadtest`
out-of-process; plus three platform-primitive scenarios: `kv-storm` (durable
read-modify-writes over the embedded redb store — the only disk-touching scenario, so
the number is the ACID-commit ceiling), `pubsub-fanout` (a publisher broadcasting 1→N
to subscriber processes — the `pubsub::Topics::publish` mechanics), and `crypto-ops`
(`crypto.subtle` SHA-256 from a sandboxed TS guest on rquickjs)). The Wasmtime backend (`rusm-wasm`, the *only* crate that
touches Wasmtime) runs each component instance-per-process via the **component
model** (`wasmtime-wasi`; `bridges/{wasip1,wasip2,wasip3}.rs` over a shared core).
The component linker wires **WASI p2 and p3** — both `@0.2.0` and `@0.3.0`
interfaces on one `WasiHost`, with the async component model enabled. It exposes a `rusm:runtime` **WIT actor world** (`bindgen!`): a
component calls `self`/`send`/`receive`/`receive-timeout` (Erlang's `receive …
after`)/`list`/`info`/`kill`/`register`/`whereis`/`set-label`/`register-tag`/`kill-tag`
(Erlang `pg` process groups — self-tag + capability-gated group kill)/`spawn`/`monitor`/
`supervise`/`stream-*`/`kv-*`/`log` — the Erlang `Process` API + durable storage +
platform logging, callable from Rust, TS, or Go guests — backed by thin calls into `rusm-otp`
(and `rusm-kv` for `kv-*`). **Logging is a platform primitive**: a guest's `console.*`
(TS) / `log` crate (Rust) routes to the host `log` op, which stamps the time,
`component#pid`, and severity colour via `rusm-logfmt` and writes to the node's log
stream — gated by `[log] level`, no `allow-stdio` and no name/pid wiring in guest code.
The serving bridges also emit a **platform access log** (`bridges/access.rs`) — every
HTTP request, SSE stream, and WS upgrade as `rusm <proto> <method> <path> → <status>`,
same stream, same `[log]` gate. (`rusm-logfmt::platform_line` is the single source for
every `rusm`-tagged line — lifecycle + access.) **Default-deny capability profiles** (`caps.rs`:
Sandboxed/NetworkClient/Trusted; grants incl. `spawn`/`process-control`/`storage`)
build a `WasiCtx` + a `StoreLimiter` memory cap. Durable **key-value storage** is
the Wasm-free **`rusm-kv`** crate (embedded redb buckets), surfaced via the `kv-*`
ABI behind the `storage` capability, with a node-level `store` and an opt-in
`WasmRuntime::with_store`. TS guests also get native **`crypto.subtle`** (RustCrypto:
SHA/HMAC/AES-GCM) in the js-runner.
The spawn path is optimized — pooling allocator + copy-on-write + per-module
`InstancePre` + **precomputed export index** + **opt-in mailbox depth** (default
off → zero hot-path atomics) + single runtime-handle clone — sustaining **~440k
component spawns/sec** (live `component-storm` scenario). Trap → process
`Crashed`. An **app model** (`rusm-cli`): `rusm new <name>` (scaffold),
`rusm.toml [components.<name>]`/`[[serve]]`, `rusm build`
(cargo `wasm32-wasip2` per `components/*`, no jco), `rusm run`/`rusm dev`/`rusm
serve`; env the Rust way (process env, then `.env` via `dotenvy`). `rusm-otp` stays Wasm-free
(verified: no `wasmtime` in its dep tree).

Underneath, the Wasm-free OTP core (`rusm-otp`) spawns,
schedules, kills, messages, supervises, manages, and **connects** **real**
lightweight processes: links, monitors, exit reasons, `trap_exit`, `spawn_link`,
`exit/2`, exit cascades, a named **registry**, **timers** (`send_after`/`cancel`),
graceful `shutdown`, **TCP** (`listen`/`connect`, one process per connection),
process **introspection** (`list`/`info`/`set_label`), and **byte streams**
(`Received::Stream`, Tokio-backpressured). Seven benchmarks are live (release):
spawn-storm (~2.4M spawns/sec), ping-pong (~21M messages/sec, round-trip p50
<1 µs), fault-recovery (~285k restarts/sec), fairness (bystanders at ~50M+
ops/sec — past 400M on free cores — under tight-loop spinners), module-storm
(~475k wasip1 core-module spawns/sec — the direct Lunatic head-to-head),
component-storm (~440k component spawns/sec), and connection-storm (thousands of
concurrent connections; connect p50 sub-millisecond). Numbers are measured under
everyday machine load and scale up with free CPU.
Each process keeps a single channel; exit signals ride the mailbox (a `Received`
enum) and kill rides a `futures` abort handle (no second signal channel — we beat
Lunatic's two). The registry is a sharded `DashMap`, timers use Tokio's timer
wheel, and TCP is process-per-connection — the connection ceiling is the OS (fds,
ports), not RUSM. Phase 0 (metrics, live observer, benchmark harness + WebSocket
server, `rusm` CLI, React dashboard, examples) is done. The **wasip1 bridge**
(`bridges/wasip1.rs`) runs preview1 core modules as processes too — preview1 WASI,
the same default-deny caps + `StoreLimiter`, the precomputed export index, and a
raw `rusm::*` actor ABI over linear memory, including **cross-process byte
streaming** (`stream_open`/`write`/`close`/`accept`/`read` over the Wasm-free
`StreamHandle`, real Tokio back-pressure) — RUSM on Lunatic's home turf
(module-storm bench). Cross-process **byte streaming** works from both core
modules (raw ABI) and **components** (the `rusm:runtime` WIT world:
`stream-open`/`write`/`close`/`accept`/`read`, handle-based). **TS/JS guests**
(Phase 8, rusm-ts core): the **js-runner** component embeds rquickjs (QuickJS,
~1.1 MB with crypto + outbound `fetch`, built with wasi-sdk) and runs a Bun-bundled JS file,
bridging a `Process` global to the actor world — a JS guest is a first-class
sandboxed process (proven by test). Like the js-http-runner, it is **wizer-pre-initialized**:
the QuickJS engine + the full JS bridge (host `__*` primitives + webapi/process/kv/rpc) are
booted once at build time and snapshotted into the image, so every spawned TS guest
CoW-starts *warm* and only evals its own bundle — no per-spawn engine/bridge boot (built via
`js-runner/build.sh`: wit-bindgen core module → wizer → `wasm-tools component new`; outbound
`wasi:http` for `fetch` is a wit-bindgen import, not the `wasip2` crate, so the artifact is a
wizer-able core module). **Phase 8 (guest ergonomics) is complete**:
**rusm-ts** (service components = exported functions; a worker = `export default`;
the concealed typed client `spawn<Svc>("svc")` with call / `for await`
streaming / callbacks / `.cast`; `rusm build` Bun→cjs; app-model loader; the
importable **`rusm-ts` npm package** for `Process`/`spawn`/types; custom capability
profiles) and **rusm-rs** (the Rust twin — `Pid`/`send`/`receive` (serde JSON) /
`spawn` / registry / `Stream` over the wit-bindgen library/binary split, plus a
`#[rusm_rs::service]` macro → dispatch loop + typed `Client` with
call/cast/streaming/callbacks, same JSON wire) and **rusm-go** (the Go peer — TinyGo →
`wasm32-wasip2`, an idiomatic `Pid`/`Send`/`Receive`/`Spawn`/`Service`+typed client/`web`
serving API over the same JSON wire; `rusm new --lang go`, `rusm build` drives TinyGo).
Rust, TS, and Go guests interoperate. All three guests get an in-guest **`Supervisor`** (one-for-one / one-for-all /
rest-for-one over a `monitor` ABI; a dead child arrives as a `__down` message — no
polling), and **`rusm dev`** watches `./components` and rebuilds + reloads on edit.
Spawn-from-guest is a capability-gated actor-ABI op: the `spawn` capability gates
*who* may spawn, and a **node-registered** component runs under **its own
manifest-declared profile** (`register_component_with`/`register_js_component_with`
store the declared caps; `actor::spawn` uses them) — what the manifest declares is
what runs, whoever spawns it, so secrets stay scoped to the component that needs them
(an ad-hoc registration with no declared profile inherits the spawner's caps). A guest
still can't fabricate capabilities the operator never granted. (The runner wraps each
bundle in a CommonJS scope so its top-level vars can't clobber the runtime globals.) **Phase 11 also closed the standard-WASI surface**: stock
**`wasi:cli/run`** command components run unchanged (`WasmRuntime::spawn_command` —
DRY-shared `build_store` with the actor path), and the TS runner gained a
capability-gated streaming **outbound `fetch`** (over `wasi:http`, gated by
`WasiHttpHooks::send_request` on the network capability — closing a latent ungated
hole) and **`crypto`** (getRandomValues/randomUUID over `wasi:random`). The **one
deferred** Phase-11 refinement is a native p3-typed `stream<u8>` WIT signature — the
handle-ABI byte streams are functionally complete and load-bearing for WS/SSE serving,
so the native signature is cosmetic standards-polish (a sweeping change to the shared
actor world deliberately not rushed). TLS folds into the Phase 9 secure cluster
transport. See
`docs/about/roadmap.md`.

## Tech stack

- **Rust** (host) + **Tokio** (scheduler/IO) + **Wasmtime** (component guests, in `rusm-wasm`).
- **Bun** for all JS/TS (dashboard, docs site) — never Node.js.
- Charts: **uPlot**. Docs site: **VitePress**.

## Conventions (please keep)

- **TDD always** — write the failing test first; baby steps.
- **Coverage: aim for 100%** (≥98% floor). Rust via `cargo-llvm-cov`; dashboard
  via `bun test --coverage`. Thin glue (`main.rs`) and presentational `.tsx` are
  excluded; only genuinely-unreachable invariant guards are acceptable gaps.
- **Comments only for critical info** — no comments restating obvious code.
- **Formatting**: `cargo fmt` + Prettier. No required linter.
- **Senior, idiomatic, reference-quality** code. Self-review every change for
  weak tests, readability, DRY, and separation of concerns.
- **Wasm-free core (hard boundary).** The Erlang/OTP core (`rusm-otp`:
  processes, messaging, supervision, registry, scheduler) must **never** depend on
  or reference Wasmtime. All Wasm lives in `rusm-wasm` (Phase 6). The distributed
  transport (`rusm-cluster`, Phase 9) is likewise Wasm-free — it sits over
  `rusm-otp` (quinn/rustls/rcgen, no Wasmtime). Wasm must not bleed into
  Wasm-irrelevant code; the dependency graph enforces it.
- **Total awareness on sweeping changes.** For any rename/renumber/API change,
  grep the *entire* repo, fix every hit, then re-grep to prove zero stragglers.

## Commands

```sh
cargo test                                  # all Rust tests
cargo llvm-cov --workspace --ignore-filename-regex 'main\.rs' --summary-only
cargo fmt --check
cargo run -p rusm-cli -- node start         # host the app as an attachable node
cargo run -p rusm-cli -- attach             # observe a node; local or host[:port]
cargo run -p rusm-cli -- new hello          # scaffold an app
cargo run -p rusm-cli -- serve              # host rusm.toml [[serve]] entries on real ports
cargo run -p rusm-bench -- start            # the benchmark/dashboard node (repo-only)
cargo run -p rusm-bench -- run connection-storm 5
cargo run -p rusm-bench --example host_components
cargo run -p rusm-loadtest -- --help        # out-of-process serving load test (vs a live `rusm serve` port)

cd bench/dashboard && bun install && bun run dev      # dashboard
cd bench/dashboard && bun test --coverage             # dashboard tests
```

## Layout

`crates/rusm-otp`, `crates/rusm-wasm`, `crates/rusm-cluster`,
`crates/rusm-kv` (Wasm-free durable redb-backed KV store),
`crates/rusm-logfmt` (Wasm-free shared log palette/format, host + wasm32-wasip2),
`crates/rusm-metrics`, `crates/rusm-observer`, `crates/rusm-node` (manifest +
profiles + the attach protocol/node), `bench/rusm-bench` (lib+bin),
`bench/rusm-loadtest` (out-of-process serving load test),
`rusm-cli` (`rusm`), `bench/dashboard` (Bun/React), `examples/`, `docs/`.
Per-crate purpose: see `README.md` → Crates.
