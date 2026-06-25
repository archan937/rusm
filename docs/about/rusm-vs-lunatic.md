# RUSM vs Lunatic — comparison & efficiency playbook

Lunatic is RUSM's closest peer — the project that first made "a Wasm instance is a
process" real, with supervisors and a distributed registry. That makes it the most
useful mirror RUSM has: studying it carefully shows both the ideas worth being inspired
by and the places where a fresh implementation, on a modern Wasmtime, can do better.
This page is two things at once — an honest, phase-by-phase comparison of where the two
runtimes stand today, and a working **efficiency playbook**: for each phase, the smart
Lunatic technique to borrow (with file evidence), why it helps, and where RUSM
deliberately aims to be faster, leaner, or more stable.

> **Living document.** RUSM is built in phases; Lunatic is a complete (but dormant,
> v0.13.0 / May 2023) runtime. We keep this doc current as each RUSM phase closes a
> gap. It exists to (a) honestly track where RUSM stands, (b) capture the smart
> Lunatic techniques worth being *inspired* by (never copied), and (c) record where
> RUSM deliberately aims to be **faster, leaner, and more stable**.
>
> Source studied: `~/Sources/lunatic` (Wasmtime 8). File references are to that tree.

## How to read this

This is not apples-to-apples. RUSM has **Phases 0–11 functionally complete** (Phase 12,
hardening, is planned), while Lunatic is a complete but **dormant** runtime. In RUSM
today: the OTP core spawns, messages, supervises, manages, and connects real processes;
the `rusm-wasm` backend runs them as real **Wasm instances** behind three bridges (wasip1
core modules, wasip2 components, wasip3 `@0.3.0`) with default-deny capabilities,
instance-per-process, and pooling + CoW + epoch; the guest crates (`rusm-rs` / `rusm-ts` /
`rusm-go`), the QUIC+TLS cluster, and HTTP/WS/SSE serving are all built. All **twenty-one**
dashboard scenarios run on real data — including **fairness** (Wasm spinners saturate
every core, yet bystanders keep progressing) and **distributed-fanout** (real cross-node
messaging over QUIC+TLS). The lasting value of this page is the **efficiency playbook**
below.

> ### Does RUSM handle lightweight processes as efficiently as Lunatic — today?
>
> **For the actor model — yes, and in places better** (one channel per process
> vs two, free abort-handle cancellation, a sharded registry, Tokio-wheel timers;
> ~2.4M spawns/sec, ~21M messages/sec, ~285k restarts/sec measured live). **For
> Wasm execution — RUSM now hosts the *component model*** (`rusm-wasm`), which
> Lunatic does **not** do at all (it runs only core modules with its own ABI):
> instance-per-process with a pooling allocator, copy-on-write init, `InstancePre`,
> a precomputed export index, and epoch preemption — **~440k component
> spawns/sec** live (component-storm). Lunatic ships on-demand allocation + fuel;
> RUSM's spawn path is ahead by design.
>
> **The direct head-to-head** is the **module-storm** scenario: RUSM spawns the
> *same artifact Lunatic hosts* — wasip1 core modules — at **~475k spawns/sec**
> live, recycling pooled instances and preempting with epochs (vs Lunatic's
> on-demand allocation + per-instruction fuel). Telling detail: that's ~the same
> as a wasip2 component (~440k/sec) — **the component model is nearly free over a
> raw core module** on RUSM's pooled path. The only big step is to a bare task
> (~2.4M/sec); that ~5x is the price of real Wasm memory isolation, paid once
> whether you run a core module or a component.
>
> The **fairness** scenario runs on real Wasm — spinners saturate every core, yet
> bystanders progress at ~50M+ ops/sec (past 400M on free cores), proving epoch
> preemption live. What
> remains is a true head-to-head against Lunatic (see the question at the end).

## Snapshot

| | RUSM (today) | Lunatic |
| --- | --- | --- |
| Status | Active, Phases 0–11 complete (12 planned) | Dormant since 2023 (v0.13.0) |
| Rust LOC | ~5,350 (6 crates) + ~790 TS | ~15,150 (20 crates) |
| Tests | ~164 Rust + 21 TS, ~99% cov | ~26 test annotations |
| Wasmtime | v45 (instance-per-process) | v8 (2023) |
| Guest target | components (`wasm32-wasip2`, WASI p2/p3) + core modules (`wasm32-wasip1`, WASI p1) | `wasm32-wasi` (preview1) |
| License | MIT | MIT + Apache-2.0 |

## Capability matrix

Implementation: ✅ done · ⚠️ partial/synthetic · ❌ not yet · 🅛 Lunatic-only · 🅡 RUSM-only
Perf/efficiency vs Lunatic: ✅ on par · 🔥 ahead by design¹ · — not built yet

This mirrors the [roadmap](/about/roadmap) **phase-for-phase** — one row per
phase, same themes, same order.

| Phase | Capability | RUSM | Lunatic | On par? (perf/efficiency) |
| --- | --- | :---: | --- | --- |
| 1 ✅ | Process & scheduler core | ✅ done (`rusm-otp`) | ✅ `WasmProcess` | ✅ on par |
| 2 ✅ | Mailboxes & message passing | ✅ done (one channel + selective receive) | ✅ selective-receive | 🔥 ahead |
| 3 ✅ | Links, monitors, supervision, fault tolerance | ✅ done (links/monitors/trap/exit) | ✅ `Signal` enum | 🔥 ahead |
| 4 ✅ | Process management (registry, timers, lifecycle) | ✅ done (sharded registry, Tokio timers) | ✅ | 🔥 ahead |
| 5 ✅ | Connectivity — TCP | ✅ done (TCP, process-per-conn; TLS shipped in P9 via QUIC) | ✅ TCP/UDP/DNS/TLS | ✅ on par |
| 6 ✅ | Wasmtime backend (instance-per-process, preemption) | ✅ done (pooling+CoW+epoch; fairness live) | ✅ (fuel) | 🔥 ahead by design |
| 7 ✅ | **Component hosting** (component model, WASI p2 + p3, capabilities, actor WIT ABI, app model) | ✅ done (~440k component spawns/s; default-deny caps + memory limits; component-storm live) | ❌ **no component-model host** (core modules only) | 🔥 **ahead — an axis Lunatic lacks** |
| ↳ 7 ✅ | …wasip1 bridge (full WASI + raw actor ABI + byte streams) + wasip3 interfaces on the component linker — *part of Phase 7, not a separate phase* | ✅ done | ✅ wasip1 | 🔥 ahead (p3 + components) |
| 8 ✅ | Guest crate | ✅ `rusm-rs` + `rusm-ts` (service macro / typed client, call/cast/stream/callbacks) | 🅛 `lunatic-rs` (Rust only) | 🔥 ahead — TS *and* Rust guests, one wire |
| 9 ✅ | Distributed clusters + live attach | ✅ `rusm-cluster` (QUIC+TLS, cross-node send, gossiped global registry, remote spawn, live attach) | ✅ (QUIC + distributed registry) | ✅ at parity — secure cluster + global registry, one persistent conn/node, message-per-stream (no custom congestion layer) |
| 10 ✅ | Scale & hardening | ✅ on-demand instance tier, bounded mailboxes, mutual-TLS cluster CA, windowed restart-intensity | ⚠️ OnDemand + fuel | 🔥 ahead — overflow tier *on top of* pooling, + secure cluster |
| 11 ✅ | **Serving** (HTTP / WS / SSE from a component) | ✅ engine built+measured, **from Rust, TS, and Go guests** — `http_server` (instance-per-request `wasi:http`), `ws_server` (one sandboxed component process per WS connection), SSE streaming body. **`rusm serve`** hosts `rusm.toml [[serve]]` entries on real ports; **`rusm new`** scaffolds an app. Six **co-resident live demos** on the dashboard (`http-throughput`/`ws-echo`/`sse-fanout` + `*-ts` twins); the **fair headline numbers** come **out-of-process** from `rusm-loadtest` (vs a live port): HTTP ~46k req/s (0% errors), WS ~146k round-trips/s (256 held), SSE ~609k events/s (256 held), ~34k sandboxed-process-per-connection WS establishments/s (`conn` mode). 0.3.0 matured it: connection context (route params), WS framing + permessage-deflate, rich SSE + Last-Event-ID, resource/CSWSH controls, compression, and native **TLS** (`https`/`wss`). | ❌ **no `wasi:http` host** (core modules only) | 🔥 **ahead — an axis Lunatic lacks** |
| 12 ⏳ | **Edge & cluster hardening** (planned) | the serve-path admission-control remainder (request-body cap + per-request timeout → graceful `503`; concurrency/message caps + CSWSH shipped in 0.3.0), default-bounded serve mailboxes, signed `name→node` gossip ownership + poison-resistant locking | ⚠️ partial | — closing network-edge & peer-trust gaps |
| — | SQLite host API | ❌ | 🅛 | — n/a |

> ¹ The perf column is an **architectural** assessment, not a head-to-head
> benchmark — and Phases 1–5 run **native** Rust bodies, so they compare the
> OTP/host machinery, *not* Wasm execution. The true lightweight-process
> efficiency race is **Phase 6**, when guests become real Wasm instances.

**Why each verdict (perf/efficiency):**

- **1 — on par.** Identical model: one process = one Tokio task. RUSM's native
  spawn sustains ~2.4M/s; Lunatic's per-spawn also instantiates a Wasm module, so
  a fair head-to-head waits for Phase 6.
- **2 — ahead.** RUSM keeps **one** channel per process (the mailbox); Lunatic
  keeps two (signal + message) and double-handles each message (mpsc →
  `Mutex<VecDeque>`). Kill rides a free abort handle — less memory, one fewer
  queue per process.
- **3 — ahead.** Exit signals ride the mailbox (no separate signal channel to
  multiplex), and a crash is captured via `std::thread::panicking()` with **no
  `catch_unwind`** per-poll cost.
- **4 — ahead.** The registry is a sharded `DashMap` (name lookups never take a
  global lock, unlike Lunatic's single `RwLock<HashMap>`); timers ride Tokio's
  hierarchical wheel instead of a hand-rolled `BinaryHeap` + one timer-service task.
- **5 — on par.** Both are process-per-connection; the connection *rate* is the OS
  kernel `connect`/`accept` ceiling (identical for both), and minting a process
  per connection is ~free on both (RUSM spawns ~100× faster than the loopback
  hands out sockets).
- **6/7 — ahead by design.** A **pooling allocator** + **copy-on-write** memory
  init + a per-module **`InstancePre`** + a **precomputed export index** sustain
  **~440k component instance-per-process spawns/s**, far ahead of a naive
  on-demand allocator, and **epoch** preemption (bumped on a dedicated thread)
  keeps bystanders at ~50M+ ops/sec (past 400M on free cores) even with a tight-loop guest pinning every
  core. Lunatic ships on-demand allocation + fuel (and no component host), so RUSM is
  ahead on both the spawn path and preemption overhead by design — a true
  head-to-head benchmark is the remaining validation.

Already shipped in Phase 0 — where RUSM already leads Lunatic:

| Capability | RUSM | Lunatic |
| --- | :---: | :---: |
| HdrHistogram latency metrics | ✅✅ | ⚠️ passthrough facade |
| Live observer + REPL attach | ✅✅ | ❌ |
| Web dashboard | ✅✅ | ❌ |
| Enforced ≥98% coverage + docs site | ✅✅ | ❌ |

---

# Efficiency playbook — phase by phase

Same order as the roadmap and the matrix above. For each phase: the smart Lunatic
techniques to **borrow** (with file evidence in `~/Sources/lunatic`), why they
help, and where RUSM **beats** them. (Borrow ≠ copy — understand, then write our
own.)

## Phase 1 — Process & scheduler core ✅

| Borrow from Lunatic | Why it helps | RUSM plan |
| --- | --- | --- |
| Biased `tokio::select!` loop, signals before the body — `lunatic-process/src/lib.rs` | deterministic signal priority, no starvation, cancellation-safe | **Ahead (Phase 2):** kill now rides a `futures` abort handle, so there's no select loop and no control channel at all |
| Single `Signal` enum over one mpsc channel — `lunatic-process/src/lib.rs` | one channel for messages *and* control — uniform | **Ahead:** RUSM keeps one channel for *messages only*; control needs none, so we removed the `Signal` type entirely |
| `HashMapId<T>` id→resource table — `crates/hash-map-id` | one uniform resource table everywhere | Adopt the *pattern*; **beat:** a slotmap / generational-index arena (array-indexed, no hashing, safe id reuse) instead of `HashMap<u64,T>` |
| Unbounded signal mailbox (`UnboundedSender`) — `lunatic-process/src/lib.rs` | Erlang-style, but unbounded → flood/memory risk | RUSM's mailbox is unbounded too (Erlang-style); **bounded + observable mailbox depth** is a later hardening option |

## Phase 2 — Mailboxes & message passing ✅

| Borrow from Lunatic | Why it helps | RUSM status |
| --- | --- | --- |
| Cancellation-safe selective receive by tag (waker + found-on-cancel re-queue) — `mailbox.rs:39-169` | safe in `select!`, no lost messages | **✅ Done** — `Context::recv_match` scans a save queue then the channel, leaving non-matches in arrival order (own code + tests) |
| ⚠️ Data messages share the single `Signal` mpsc with control | a message flood can delay `Kill`/`Link` handling (FIFO within one channel) | **✅ Ahead** — control (kill) rides a free abort handle, so messages have the mailbox entirely to themselves; *zero* control channels vs Lunatic's shared signal mpsc |
| ⚠️ Two queues per message (signal mpsc → `Mutex<VecDeque>`) + a `Vec<u8>` per message | double handoff + an allocation per message | **✅ Ahead** — one mailbox queue per process, no double handoff; small-message inlining (smallvec) and buffer pooling remain a later option |
| `DataMessage{buffer, resources: Vec<Arc<Resource>>}` — resources moved by `Arc`, only bytes copied — `message.rs:68-103` | zero-copy handoff of sockets/modules | **Planned** — Phase 2 carries opaque bytes (pids encoded inline); first-class typed resources land with the host ABI (Phase 6) |
| Address peers via held handles — no global-table lookup on `send` — `lunatic-process/src/lib.rs` | the send hot path never locks a global table | **Partial** — `send` goes through a *sharded* `DashMap` (no global lock, unlike our old `Mutex<HashMap>`); pure handle addressing is a later option |

## Phase 3 — Links, supervision, fault tolerance ✅

| Borrow from Lunatic | Why it helps | RUSM status |
| --- | --- | --- |
| `Signal::{Link,Monitor,LinkDied}` + `die_when_link_dies` — `lunatic-process/src/lib.rs` | unified, configurable supervision | **✅ Done, ahead** — `link`/`monitor`/`trap_exit`/`spawn_link`/`exit`, but exit signals ride the *mailbox* (a `Received` enum) and kill rides the abort handle, so there's still **no separate signal channel** to multiplex |
| trap → `ResultValue::Failed` → `LinkDied` propagation — `runtimes/wasmtime.rs` | a crash notifies linked peers | **✅ Done** — a crash is caught via `std::thread::panicking()` in the teardown guard (no `catch_unwind`, no per-poll cost); the abnormal reason cascades down links and is *staged* so a cascaded peer reports the original reason, not a bare kill |

## Phase 4 — Process management ✅

| Borrow from Lunatic | Why it helps | RUSM status |
| --- | --- | --- |
| Named registry `Arc<RwLock<HashMap>>` — `lunatic-registry-api` | async-safe name → pid | **✅ Done, ahead** — a sharded `DashMap` registry (name lookups never take a global lock), with names auto-released on process exit |
| Timers: `BinaryHeap` + `HashMapId` (O(log n) cancel) — `lunatic-timer-api` | cheap cancellation of many timers | **✅ Done, simpler** — `send_after` rides Tokio's hierarchical timer wheel and cancellation is a free `AbortHandle`, so there's no hand-rolled heap and no single timer-service bottleneck |

## Phase 5 — Connectivity (TCP) ✅

| Borrow from Lunatic | Why it helps | RUSM status |
| --- | --- | --- |
| Process-per-connection accept loop — `lunatic-networking-api` | a slow/crashing connection can't affect the others | **✅ Done** — `Runtime::listen` spawns one rusm-otp process per accepted socket; the connection ceiling is the OS (fds, ephemeral ports, TIME_WAIT), not RUSM, since spawning is ~free (the spawn storm does 2.4M/s) |
| TCP owned read/write halves + per-conn timeouts in `HashMapId` — `lunatic-networking-api/src/lib.rs:71` | concurrent reader+writer without locking the stream | **Deferred** — native handlers own the whole `TcpStream`; split halves / per-conn timeouts arrive with the guest host ABI (Phase 6) |
| TLS via `tokio-rustls` + `webpki-roots` — `tls_tcp.rs:392` | secure transport | **Moved to Phase 9** — TLS's real home is the secure cluster transport (QUIC + TLS); bolting it onto the loopback storm would only tank throughput |

## Phase 6 — Wasmtime backend ✅  ← the biggest efficiency win

| Borrow from Lunatic | Why it helps | RUSM status |
| --- | --- | --- |
| `InstancePre` (imports type-checked once) + Arc'd `Module` + async — `runtimes/wasmtime.rs:34,63,163` | fast instantiation, compile-once | **✅ Done** — the linker is built once and each module's imports resolve once into an `InstancePre`; a spawn skips import resolution (and a precomputed export index skips the by-name lookup) — part of the path to ~440k component spawns/s |
| `InstanceAllocationStrategy::OnDemand` — `runtimes/wasmtime.rs:173` | fresh memory per instance → slower, heavier spawn | **✅ Ahead** — pooling allocator (pre-reserved slabs) → spawns reuse slots, no mmap (a large multiple of on-demand allocation) |
| `static_memory_forced(true)` on v8 (no CoW) — `runtimes/wasmtime.rs:175` | static memories, but no copy-on-write | **✅ Ahead** — `memory_init_cow`: a fresh instance shares the module image until first write, so init is near-free |
| Preemption via **fuel** (`consume_fuel`, `out_of_fuel_async_yield`) — `runtimes/wasmtime.rs:166,56` | works, but per-unit accounting overhead | **✅ Ahead** — epoch interruption: a periodic atomic bump, ≈ near-zero steady-state; an infinite-loop guest still yields and stays killable |

**Why this phase matters most:** pooling + CoW + epoch + `InstancePre` + a
precomputed export index are exactly the levers for cheap instance-per-process.
They're in (`rusm-wasm`), giving ~440k component spawns/s, and the fairness
scenario proves epoch preemption live. Lunatic (Wasmtime 8, on-demand, fuel,
core-modules-only) predates easy access to them — the remaining work is
a true head-to-head benchmark to put numbers on the delta.

## Phase 7 — WASI + per-process sandbox

| Borrow from Lunatic | Why it helps | RUSM plan |
| --- | --- | --- |
| WASI preopens (scoped fs), isolated per-process stdio — `lunatic-wasi-api` | fine-grained filesystem sandbox | Adopt; **beat:** finer memory/fuel limits per process |
| stdout capture as a `WasiFile` — `lunatic-stdout-capture` | isolated, inspectable output | Adopt — feeds the observer/attach |

## Phase 8 — Guest crate

| Borrow from Lunatic | Why it helps | RUSM plan |
| --- | --- | --- |
| `lunatic-rs` API shape — spawn / `Mailbox` / `AbstractProcess` / `Supervisor` (separate repo) | a familiar, ergonomic guest API | ✅ `rusm-rs` *and* `rusm-ts` ship `Pid`/`send`/`receive`/`spawn`/`Stream`, a `#[service]` macro (typed `Client`), and an in-guest `Supervisor` (one-for-one / one-for-all / rest-for-one) |

## Phase 9 — Distributed clusters + live attach

| Borrow from Lunatic | Why it helps | RUSM plan |
| --- | --- | --- |
| One persistent QUIC conn/node + N-stream pool (`NodeConnectionManager`) — `congestion/mod.rs:174` | 1 TLS handshake/node, multiplexed | Adopt |
| Deterministic stream routing `((src ^ dest) % streams)` — `congestion/mod.rs:244` | in-order per process-pair, no head-of-line block | Adopt |
| ⚠️ Custom 1 KiB chunking + a congestion-control worker, *on top of QUIC* — `congestion/mod.rs:69,99` | QUIC already gives reliable ordered streams with per-stream + connection **flow control and congestion control** — re-implementing it is redundant complexity | **Reconsider:** length-prefixed framing over QUIC streams and let QUIC apply backpressure; add app-level chunking only if profiling shows real head-of-line / fairness issues |
| Atomic message IDs + `DashMap` response cache + `AsyncCell` — `distributed/client.rs:85,93` | lock-free RPC hot path; sharded reads | Adopt |
| `rmp-serde` (MessagePack) wire format — `distributed/Cargo.toml` | compact + fast vs JSON | Adopt (or `bincode`/`postcard`; benchmark) |
| Cert-embedded authz (X.509 ext, OID 2.5.29.9) + 100 ms keep-alive — `quic/quin.rs:61,145` | auth at handshake, NAT traversal | Adopt |
| Node discovery via 5 s HTTP polling — `control/client.rs:287` | simple, but laggy for fast scaling | **Beat:** push-based discovery + pre-warmed connections |

## Phase 10 — Performance & hardening

Roll up the "beat" levers and prove them: pooling + CoW + epoch (already at ~440k
component spawns/sec, ~475k wasip1 core-module spawns/sec — the direct head-to-head),
quinn 0.11+ with adaptive chunking, and the superiority scorecard below — each as a
measured number on the dashboard.

---

## Superiority scorecard (recap)

A one-glance summary of the **beat** items above — all **targets to validate on
the dashboard**, not yet achieved.

| Dimension | Lunatic | RUSM target | Lever (phase) |
| --- | --- | --- | --- |
| Spawn throughput | OnDemand alloc | **higher** | pooling + CoW (6) |
| Memory / process | fresh memory per instance | **lower** | CoW-shared pages, pooled slots (6) |
| Scheduling overhead | fuel accounting | **lower** | epoch interruption (6) |
| Engine | Wasmtime 8 (2023) | **current** | modern Cranelift/async/CoW (6) |
| Connectivity | quinn 0.10, fixed chunks, 5 s poll | **lower latency** | quinn 0.11+, adaptive chunks, push discovery (9) |
| Stability under flood | unbounded mailbox | **bounded + observable** | depth limits + live observer (1) |
| Observability | metrics facade | **first-class** | HdrHistogram + observer + REPL (Phase 0, shipped) |

## Critical review — where Lunatic looks improvable

Being honest about the source we admire: beyond version-currency, several *design*
choices look improvable. Each is an opportunity to **evaluate with its trade-off**,
not a settled verdict — and several only matter at scale.

| Lunatic choice | The critique | Better opportunity (phase) |
| --- | --- | --- |
| Custom chunking + congestion worker over QUIC | re-implements flow/congestion control QUIC already provides — extra code + overhead | length-prefixed framing over QUIC streams; let QUIC do backpressure (9) |
| Messages + control share one `Signal` mpsc | control (`Kill`/`Link`) can sit behind a data-message flood | separate high-priority control channel (2) |
| Resource tables are `HashMap<u64,T>` | hashing + pointer-chasing per access; ids never reused | slotmap / generational arena — array-indexed, cache-friendly, id reuse (1) |
| Two queues + a `Vec<u8>` per message | double handoff + per-message allocation | one queue; inline small messages + buffer pool (2) |
| `rmp-serde` for intra-cluster RPC | schemaless format on a both-ends-RUSM link leaves speed on the table | zero-copy / codegen format (`postcard`, `rkyv`) for the internal wire (9) |
| stdout capture = `Arc<RwLock<Vec<Mutex<Cursor>>>>` | nested locks — complexity & contention smell | per-process ring buffer, or a single writer task fed by a channel (7) |
| `Arc<dyn Process>` dynamic dispatch on the hot path | vtable indirection where the process kind is known | concrete/enum process type; reserve `dyn` for remote proxies (1–2) |

### Dated / version pitfalls (don't inherit)

- **Wasmtime 8** + **quinn 0.10** — well behind current; upgrading buys CoW, pooling, flow-control.
- **OnDemand allocation** + **fuel** preemption — leave spawn / memory / scheduling wins on the table.
- **`Mutex` on read-heavy timeout fields** → `RwLock`. **Fixed 1 KiB chunks** + **5 s HTTP polling** → adaptive / push.
- **No TLS session resumption**; **unbounded signal mailbox** (flood risk).

> Caveat (intellectual honesty): Lunatic is battle-tested and shipped; some of
> these are deliberate simplicity/portability trade-offs, and a few "wins" only
> show up at high scale. We validate each on the dashboard before claiming it.

# Maintaining this document

Update at the end of each phase: flip the matrix cells RUSM now implements, record
the **measured** spawn/sec, memory/process, and latency vs the targets above
(screenshot or numbers from the dashboard), and note any technique we adopted,
modernized, or rejected — with the reason.
