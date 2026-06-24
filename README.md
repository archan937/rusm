# RUSM

**An Erlang-inspired WebAssembly runtime in Rust.**

RUSM gives you isolated, lightweight *processes* — message passing, supervision,
fault tolerance, and secure clusters you can hook into live — the BEAM's
concurrency and connection model, in Rust. The **Erlang/OTP actor model is the
core** (pure Rust); **WebAssembly is the sandboxed execution backend** that later
runs each process as an isolated instance. Rust + Tokio do the scheduling;
Wasmtime does the isolation.

> **Status:** Phases 0–11 of 12 are functionally complete (the native `stream<u8>`
> WIT signature is the one deferred refinement); Phase 12 (edge & cluster hardening)
> is planned. See the [roadmap](docs/about/roadmap.md).

## Prerequisites

- **Rust** 1.94+ — [rustup](https://rustup.rs).
- **Bun** 1.3+ — [bun.sh](https://bun.sh) (builds TypeScript components; runs the dashboard).
- **Go + TinyGo** 0.41+ — *optional*, only to build **Go** components; `mise install` pins
  the full toolchain (TinyGo, wit-bindgen-go, binaryen, wasm-tools — see `mise.toml`).

## Quick start

Install the CLI — no clone needed (you only clone to hack on RUSM itself or run its
dashboard):

```sh
cargo install rusm-cli            # the `rusm` command
rustup target add wasm32-wasip2   # only to build Rust guest components
```

**Build your own — from nothing to a live server in four commands:**

```sh
rusm new hello && cd hello   # scaffold a TS HTTP component + rusm.toml
rusm build                   # components/ → wasm/
rusm serve                   # → http://127.0.0.1:8080
curl http://127.0.0.1:8080/  # "Hello from RUSM 👋"
```

### Scaffold a real app — the TODO board

A collaborative TODO board (HTTP CRUD + a live SSE feed + WebSocket Chat + a resident
`store` service driven by a worker), in TypeScript, Rust, or Go:

```sh
rusm new board --template todo-board   # add --lang rust or --lang go (default: ts)
cd board && rusm build && rusm serve   # → open http://127.0.0.1:8080
```

## Live benchmarks dashboard

To watch the runtime flexed live, clone the repo and run the dashboard:

```sh
git clone https://github.com/archan937/rusm.git && cd rusm
make dashboard   # builds + starts a node, launches the dashboard, opens your browser
```

Pick a scenario (spawn-storm, stream-pipe, the HTTP/WS/SSE serving tiles, …), hit
**Run**, and watch real throughput, latency percentiles, and the host/instance
observer — every scenario runs a real engine (headline numbers in
[Benchmarks](#benchmarks) below).

## A taste

A *service* is just exported functions; call it from another sandboxed process and the
cross-process round-trip reads like a local call — fully typed, even across languages:

```ts
// commander.ts — talks to the `calc` service running in its own WASM process
import { spawn } from "rusm-ts";
import type { Calc } from "../calc";

const calc = spawn<Calc>("calc");        // start a fresh process (connect<Calc>("calc") attaches to a running one)
console.log(await calc.add(2, 3));       // 5 — a real message round-trip, reads local
for await (const n of calc.countTo(3))   // generator handlers stream back
  console.log(n);                        // 1, 2, 3
```

Write the same service in Rust with [`rusm-rs`](crates/rusm-rs) — a Rust client and a
TS service interoperate over one wire. Each process is a **sandboxed WASM instance**,
supervised and hot-reloadable, and you never write `async` in a guest. Or skip
components entirely and use the pure-Rust [`rusm-otp`](crates/rusm-otp) core directly.

**New here?** The [Getting Started guide](docs/introduction/install.md) walks from the
pure-Rust OTP core to hosting a `.wasm`, the app model, and writing components in
TypeScript and Rust — then the [Concepts](docs/) and the
[`rusm` CLI reference](docs/build-an-app/the-rusm-cli.md). `make help` lists every dev command.

## Why

Existing options force a trade-off. The BEAM has the model we want (cheap
processes, "let it crash", distribution, live introspection) but only runs BEAM
languages. WebAssembly component-model runtimes give language portability but a
heavy, rigid wiring model and no actor semantics. RUSM takes the BEAM's ideas,
builds them in pure Rust, and uses WebAssembly purely as the per-process sandbox:

- **Isolated processes** — each process gets its own stack, heap, and (with the
  Wasm backend) its own sandboxed instance. A crash stays contained.
- **Massive concurrency** — processes are Tokio tasks scheduled M:N over a few OS
  threads. The goal is hundreds of thousands of spawns per second.
- **Write blocking code, get async for free** — Wasmtime fibers suspend a guest's
  "blocking" call while the host awaits; you never write `async` in a guest.
- **Fault tolerance** — links and supervisors, Erlang-style.
- **Secure clusters you can hook into** — nodes connect over TLS, and you can
  attach a live REPL/observer to a running node (like `iex --remsh`).

See [`docs/introduction/why-rusm.md`](docs/introduction/why-rusm.md) for the full rationale,
[`docs/about/architecture.md`](docs/about/architecture.md) for how Rust + Tokio +
Wasmtime map onto the BEAM, and the [RUSM vs Lunatic
comparison](docs/about/rusm-vs-lunatic.md) for where we borrow and where we aim to
beat the runtime that inspired this.

## What's there

- **A Wasm-free OTP core** (`rusm-otp`) — real lightweight processes: `spawn`,
  message passing, links / monitors / `trap_exit`, supervision, a named registry,
  timers, graceful shutdown, and TCP (one process per connection).
- **Wasmtime as the per-process sandbox** (`rusm-wasm`) — instance-per-process behind
  three bridges: **wasip1** (core modules + a raw `rusm::*` ABI), **wasip2** (the
  component model + the `rusm:runtime` WIT actor world — the Erlang `Process` API in
  any language), and **wasip3** (`@0.3.0` async WASI). Default-deny capabilities
  (fs / net / env / memory / spawn) and epoch preemption.
- **Guests in Rust, TypeScript, or Go** — a *service* is just exported functions, called
  through a concealed typed client (`spawn<Svc>("svc")` → `await svc.method(…)`, with
  streaming + callbacks), an in-guest `Supervisor`, and `rusm dev` watch + reload. The
  [`rusm-rs`](crates/rusm-rs) crate, [`rusm-ts`](packages/rusm-ts) npm package, and
  [`rusm-go`](packages/rusm-go) SDK share one wire and interoperate.
- **An app model** — `rusm.toml [components.<name>]`, source under `components/`, built to
  `./wasm/`, spawned under their capabilities; env the Rust way (process env, then
  `.env`).
- **Serving** — a component runs as a high-throughput **HTTP / WS / SSE** server
  (`rusm serve` + `rusm.toml [[serve]]`), and `rusm new <name>` scaffolds a
  ready-to-serve app. Guests get a capability-gated, streaming **`fetch`** + **`crypto`**.
- **Distributed clusters** (`rusm-cluster`) — nodes connect over **QUIC + TLS**:
  cross-node `send`, a gossiped global registry, remote spawn, and live attach.
- **Hardened for scale** — an on-demand instance tier (bounded by RAM, not a fixed
  pool), opt-in bounded mailboxes, per-node certs under a cluster CA + mutual TLS, and
  windowed supervisor restart-intensity.

## Benchmarks

Twenty-one dashboard scenarios run on **live data** — release builds, measured under
everyday machine load; they scale up with free CPU.

| Scenario | Result |
| --- | --- |
| spawn-storm | ~2.4M spawns/sec |
| ping-pong | ~21M msgs/sec · round-trip p50 < 1 µs |
| fault-recovery | ~285k restarts/sec |
| fairness | bystanders ~50M+ ops/sec (past 400M on free cores) |
| module-storm (wasip1 core modules) | ~475k spawns/sec |
| component-storm | ~440k spawns/sec |
| stream-pipe | multiple GB/sec between processes |
| connection-storm | thousands concurrent · connect p50 sub-ms |
| distributed-fanout (QUIC + TLS) | ~550k cross-node msgs/sec · ~39 µs p50 |

**Serving** headline numbers are measured **out-of-process** by `rusm-loadtest`
against a live `rusm serve` port (loopback): HTTP **~46k req/s** (0% errors) · WS
**~146k round-trips/s** (256 held) · SSE **~609k events/s** (256 held) · **~34k**
sandboxed-process-per-connection WS establishments/sec. The six serving dashboard
tiles are co-resident live demos (in-process server + load generator on one node).

Three **platform-primitive** tiles continue the set: `kv-storm` (durable
read-modify-writes over the embedded redb store — the only disk-touching scenario, so
its number is the ACID-commit ceiling), `pubsub-fanout` (one publisher broadcasting
1→N to subscriber processes), and `crypto-ops` (`crypto.subtle` SHA-256 from a
sandboxed TypeScript guest). Two **extension-primitive** tiles round out the twenty-one:
`custom-bridge` (a sandboxed guest calling an app-registered native bridge in a loop —
the custom-bridge call cost) and `dynamic-wasm` (a runtime-loaded compiled component
spawned through the content-addressed compile cache — compiled once cold, then hot).

## Configuration

Your app's `rusm.toml` declares what to run: `[[serve]]` (HTTP/WS/SSE listeners — each
a **pure listener**, no handler or capability of its own), each routed HTTP/SSE listener
carrying its own `[serve.routes]` subtable (declarative
`"METHOD /path/:param" = "component#action"` routing, so multiple listeners route
independently; a WS or routes-less HTTP listener names its single handler with `name`),
`[components.<name>]` (keyed by name — registered for spawn-by-name; the routes' handlers
live here and carry their **own** capability, while `resident = true` entries are
boot-spawned + supervised long-lived services), and custom `[capabilities.<name>]` profiles
(default-deny). Serving is always
process-per-request (HTTP/SSE) / process-per-connection (WS) — a fresh sandboxed
instance per unit of work, so head-of-line blocking is impossible by construction and
a crash drops only that request; shared state lives in a `[components.<name>]` service
(`resident = true`) or `kv`, never in the serving instance. A Rust handler is just named functions —
`#[rusm_rs::handlers] pub mod api { pub fn home(req, params) -> Response { … } }` (SSE
and WebSocket are per-connection `sse::serve` / `ws::serve` handlers) — no `main`, no router code. Env
is resolved the Rust way — process env first, then `.env`. Full reference:
**[configuration](docs/build-an-app/configuration)**.

> The benchmark/dashboard node (`rusm-bench start`, a repo-only tool) has its own,
> separate `[node]` knobs — `listen`, `profile` (`light`/`balanced`/`max`),
> `ticks_per_second` — set via `rusm.toml`/`--config`/flags and switchable live
> from the dashboard ([details](docs/about/benchmark-dashboard-and-observer.md)).

## Running tests

```sh
make test       # all Rust tests + dashboard tests
make cov        # coverage (gate: >= 98%, mostly 100%)
make fmt-check  # cargo fmt + Prettier
```

…or directly: `cargo test`, `cargo test -p rusm-otp`, and
`cd bench/dashboard && bun test --coverage`.

## Docs site

The documentation under `docs/` is also a [VitePress](https://vitepress.dev) site
(landing page, sidebar, search, dark mode):

```sh
make docs        # live preview   (or: make docs-build for the static site)
```

## Crates

RUSM is a Cargo workspace; each crate has a single job. The core is **Wasm-free
by construction** — Wasm lives only in the `rusm-wasm` backend.

| Crate | Kind | Purpose |
| --- | --- | --- |
| `rusm-otp` | lib | **The Erlang/OTP core** — processes, scheduler, mailboxes & selective receive, links/monitors/supervision, registry, timers, TCP. Pure Rust, **no `wasmtime` dependency** (usable standalone). Built up across Phases 1–5. |
| `rusm-wasm` | lib | **The Wasmtime backend** — the *only* crate that touches Wasmtime; runs each process as a sandboxed Wasm instance behind three bridges (`wasip1` core modules + raw actor ABI + byte streams, `wasip2` components + WIT actor world, `wasip3` `@0.3.0` interfaces), with default-deny capabilities, epoch preemption, pooling + CoW — all behind the same `rusm-otp` API. |
| `rusm-cluster` | lib | **The distributed transport** (Phase 9) — connects nodes over QUIC + TLS for cross-node `send`, a gossiped global registry, remote spawn, and live attach. Over `rusm-otp`, **no `wasmtime` dependency**. |
| `rusm-kv` | lib | **The durable key-value store** — embedded, transactional buckets over `redb` (pure-Rust, ACID, no daemon). Surfaced to guests by `rusm-wasm` behind the `allow-storage` capability (the `kv-*` ABI). Like `rusm-otp`, **no `wasmtime` dependency**. |
| `rusm-metrics` | lib | Counters, HdrHistogram-backed latency percentiles, ring-buffer time-series. |
| `rusm-observer` | lib | Low-overhead live-observer snapshots — aggregate counters plus a sampled per-instance table, with a detail on/off toggle. |
| `rusm-logfmt` | lib | **The shared log format** — the single source for the log palette, column widths, timestamp, and tty-gated colour. Formats both `rusm-otp`'s lifecycle lines and every **guest** log line (the host's `log` op stamps a guest's `console.*` / `log::*` through `line()`), so platform and app logs read as one aligned stream. Pure (std only); compiles for the host and `wasm32-wasip2`. **No `wasmtime` dependency.** |
| `rusm-node` | lib | **The node layer** — the `rusm.toml` app manifest, resource-tier profiles, and the live **attach** protocol + node (streams `rusm-otp` process introspection to `rusm attach`). What the `rusm` CLI builds on; **no `wasmtime` dependency**. |
| `rusm-bench` | lib + bin | *(repo-only, unpublished)* Scenarios, the deterministic preview source, twenty-one real engines — the ten core engines (spawn-storm, ping-pong, fault-recovery, connection-storm, connection-scale, fairness, module-storm, component-storm, stream-pipe, distributed-fanout), the six co-resident serving demos (`http-throughput`, `ws-echo`, `sse-fanout` and their `*-ts` twins, each a real in-process WASM server driven through the same load path as `rusm-loadtest`), three platform-primitive scenarios (`kv-storm` durable read-modify-writes over redb, `pubsub-fanout` 1→N broadcast, `crypto-ops` `crypto.subtle` from a TS guest), and two extension-primitive scenarios (`custom-bridge` a guest calling an app's native bridge, `dynamic-wasm` a runtime-loaded compiled component through the compile cache) — the run aggregator, the wire protocol, and the WebSocket node behind the dashboard. Binary: `rusm-bench start` / `run`. |
| `rusm-loadtest` | bin | *(repo-only, unpublished)* **Out-of-process serving load test** — drives a live `rusm serve` port across a real socket in four modes: `http` (balter fixed-rate sweep), `ws` / `sse` (a tokio-native connection-capacity harness), and `conn` (a connection-establishment storm — sandboxed-process-per-connection WS establishments). Reports achieved throughput, tail latency, and error rate. |
| `rusm-cli` | bin (`rusm`) | The `rusm` command: `new <name>` (scaffold an app), the app model — `build` / `run` / `serve` / `dev` over `rusm.toml` — plus `node start` (host the app as an attachable node) and `attach <url>` (observe a running node's processes). |
| `rusm-rs` | lib (guest) | **The Rust guest crate** — write a component/service in Rust over the actor world: `Pid`/`send`/`receive` (serde, + `receive_timeout`)/`spawn`/registry/`Stream`, the `#[rusm_rs::service]` macro (dispatch loop + typed `Client`), plus modules for serving (`http`/`ws`, incl. offloaded SSE fan-out), durable storage (`kv`), pub/sub (`pubsub::Topics`), and a `log`-crate sink (`log::info!`/etc → the platform logger, installed by the entry macros). Wasm-only (built for `wasm32-wasip2`), excluded from the host workspace. |
| `rusm-rs-macros` | proc-macro | The `#[rusm_rs::service]` macro behind `rusm-rs`. |

Not crates: the dashboard at `bench/dashboard` (Bun/React); docs under `docs/`. The
**rusm-ts** guest (TS/Bun) ships as the embedded js-runner in `rusm-wasm/js-runner`; the
**rusm-go** guest SDK (Go, compiled via TinyGo) lives at `packages/rusm-go`.

## Examples

`examples/` holds ready-to-run programs in three groups:

| Group | What it is |
| --- | --- |
| [`todo-board/typescript`](examples/todo-board/typescript) · [`todo-board/rust`](examples/todo-board/rust) · [`todo-board/go`](examples/todo-board/go) | **Apps — start here.** The same collaborative TODO board (HTTP + SSE + WS + a service), one per guest language. `cd examples/todo-board/<lang> && rusm build && rusm serve`. |
| [`embedding/`](examples/embedding) | **Use RUSM as a Rust library** — host components, embed a node, run a TS guest, build a cluster. |
| [`benchmarks/`](examples/benchmarks) | **Performance, measured not asserted** — HTTP/WS/SSE serving, connection scale, cross-node fan-out, against real baselines. |

See [`examples/README.md`](examples/README.md) for the full index and end-to-end recipes.

## Acknowledgements

My Elixir years left me with a clear itch: I wanted Elixir's concurrency and
process model, but in Rust, running WebAssembly — on infrastructure that's
lightweight, optimal, and *crazy fast*.
[**Lunatic**](https://github.com/lunatic-solutions/lunatic) by Bernard Kolobara
nailed exactly that pitch — its whole message and how it profiled itself was spot
on (Wasmtime + Tokio + stack switching, processes as Wasm instances). Honestly,
**if Lunatic were still active and up to date, RUSM would never have been built**
— I'd just use it. RUSM exists to carry that torch forward.

## License

MIT © [Paul Engel](https://github.com/archan937)
