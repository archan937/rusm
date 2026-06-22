# Benchmark dashboard & observer

The dashboard (`bench/dashboard`, React on Bun, uPlot charts) is the north-star
artifact — how we *see* every phase's progress. It has two views fed by one
WebSocket stream from a node.

## Running it

The dashboard lives in the repo, so clone and run it with `make`:

```sh
git clone https://github.com/archan937/rusm && cd rusm
make dashboard      # builds + starts a node, then the dashboard — open the printed URL
```

Pick a scenario (e.g. **spawn storm**, **stream pipe**), hit **Run**, and watch real
throughput, latency, and the live observer — everything driven by the real runtime.

| Command | What it does |
| --- | --- |
| `make dashboard` | Build + start the benchmark node, then the dashboard (the headline demo). |
| `make node` | Start the benchmark node on `ws://127.0.0.1:4000` (release). |
| `make ui` | Start only the dashboard (expects a node already running). |
| `make run SCENARIO=… SECONDS=…` | Run a benchmark scenario in the terminal. |
| `make example EX=…` | Run a bundled example (`host_components`, …). |
| `make test` / `make cov` | All Rust + dashboard tests / coverage. |
| `make fmt` / `make fmt-check` | Format / check (Rust + dashboard). |
| `make docs` / `make docs-build` | Live-preview / build this docs site. |

Run `make` with no target for the full list.

## Benchmark view

A menu of scenarios and a **Run** button. Each tick streams: throughput
(ops/sec), peak concurrent, latency p50/p95/p99, and a live throughput chart.

| Scenario | Headline | Real after |
| --- | --- | --- |
| Spawn storm | spawns/sec, memory/process | phase 1 |
| Message ping-pong | msgs/sec, round-trip latency | phase 2 |
| Fault recovery | restarts/sec, recovery latency | phase 3 |
| **Connection storm** | conns/sec, peak, latency | phase 5 (native), re-measured phase 6 |
| Connection scale (held-open to the fd ceiling) | peak concurrent connections | phase 5 |
| Fairness under tight loop | bystanders keep progressing | phase 6 |
| Module storm (wasip1, Lunatic head-to-head) | core-module spawns/sec | phase 6 |
| Component storm | component spawns/sec | phase 7 |
| Stream pipe | bytes/sec between processes | phase 7 |
| Distributed fan-out | cross-node latency | phase 9 |
| HTTP throughput + `-ts` twin | req/sec (co-resident live demo) | phase 11 |
| WS echo + `-ts` twin | round-trips/sec (co-resident live demo) | phase 11 |
| SSE fan-out + `-ts` twin | events/sec (co-resident live demo) | phase 11 |
| KV storm | durable read-modify-writes/sec (ACID commits, redb) | phase 11 |
| Pub/sub fan-out | subscriber deliveries/sec (1 publish → N) | phase 11 |
| Crypto ops (TS) | `crypto.subtle` SHA-256 digests/sec | phase 11 |
| Custom bridge | native custom-bridge calls/sec (guest → host round-trip) | phase 11 |
| Dynamic WASM | dynamic component spawns/sec (compiled once, then cached hot) | phase 11 |

All **twenty-one** scenarios above run **real** engines — none are synthetic
(`Runner::start_synthetic` keeps a runtime-free deterministic preview only for UI
development). The **six serving scenarios are co-resident live demos**: each spins up
the same real in-process WASM server and drives it through the same load path as the
out-of-process `rusm-loadtest` (balter for HTTP request-rate, a connection-capacity
harness for WS/SSE held connections), with the load generator and server sharing the
node process. Because they share CPU and hide the network behind loopback, the
**fair, credible headline numbers** for serving are still the ones measured
**out-of-process** by `rusm-loadtest` against a live `rusm serve` port (see [serving
HTTP/WS/SSE](/deep-dive/serving-http-ws-and-sse)). The runtime micro-benchmarks (spawns/sec,
msgs/sec, restarts/sec, scheduler fairness) stay **in-process** on purpose — they
measure the actor core itself where there is no network/server, so in-process is the
correct way to measure raw runtime capacity. The three **platform-primitive**
scenarios are likewise honest in-process measurements of the capabilities a real app
leans on: **KV storm** drives durable read-modify-writes against the embedded
`rusm-kv` (redb) store — the only scenario that touches disk, so its number is the
ACID-commit ceiling (writers serialise behind one commit lock; readers are concurrent
MVCC); **pub/sub fan-out** is one publisher broadcasting to N subscriber processes
(the exact mechanics of `rusm-rs` `pubsub::Topics::publish`); and **crypto ops** runs
`crypto.subtle` (native RustCrypto) inside a sandboxed TypeScript guest, so its rate
is the honest cost of offering Web Crypto from a JS component.

## Live observer view

A real-time view of the node (à la Erlang `observer`): process count,
running/waiting, scheduler load bars, total memory, and a per-instance table.

## Observability must stay cheap

Counters are relaxed atomics; the node pushes a **periodic aggregated snapshot**
(10–60 Hz), never an event per operation. The per-instance detail table is the
only costly part of a snapshot, so it is **toggleable** — off for clean
benchmark runs. We prove the overhead is negligible by running a high-rate
benchmark **observer-on vs observer-off** (toggle it live with `detail off` in `rusm attach`, or the dashboard's detail switch).

## Spawn-storm: how the number is produced (read this)

The spawn-storm is the first scenario on **real** data, so it's worth being
precise about what its `ops/sec` means and why it's safe.

- **It's a continuous, multi-core storm.** One background spawner task per
  (allowed) core hammers `rusm-otp` — `runtime.spawn(...)` — as fast as it can.
  A single sequential loop would be capped by one core; a storm uses many. The
  tick just **samples** the achieved rate (`Δspawned / Δt`).
- **It measures create *and reap*.** The spawned processes are trivial and finish
  immediately, so the rate reflects full lifecycle throughput, not just creation.
- **Backpressure is a safety net, not the operating point.** Spawners pause if
  the *live* population ever reaches the in-flight cap, so the table can't grow
  without bound. But at every profile the population **self-limits far below the
  cap** (a few hundred live), because spawn rate and reap rate balance out — so
  "peak concurrent" reflects the real steady-state population, not a configured
  ceiling.
- **Throughput is reap-bound, so the lever is the spawner-to-reaper balance.**
  The limit is how fast finished processes drain (~one reaper core's worth each).
  Too few spawners under-drives the machine; too many starve the reapers and pile
  processes up *without* going faster. The sweet spot is spawners ≈ reapers
  (~half the cores each) — that's what **Max** uses for peak *smooth* throughput.
- **`memory` shows 0.** Native processes have no per-instance linear memory; that
  figure becomes real once processes are Wasm instances (Phase 6).

## Resource profiles (the throughput dial)

A segmented control picks how hard the storm drives the machine. The **spawn
worker count is the dial** and is relative to your CPU count; throughput rises
with each tier. The in-flight cap is a uniform per-core safety net (memory can't
run away), **not** a per-tier knob — the population self-limits well below it.

| Profile | Spawn workers | Throughput (busy 10-core box, release) | Use it when |
| --- | --- | --- | --- |
| **Light** | ~¼ of cores | ~2.1M/s | speed isn't the point — leave the machine alone |
| **Balanced** (default) | ~⅖ of cores | ~2.4M/s | good throughput with visible room above |
| **Max** | ~½ of cores | ~2.8M/s | most performant — peak sustained rate, still smooth |

`Max` deliberately stops at ~half the cores: the other half reap, which is the
sustained-throughput peak. Pushing spawners higher does **not** go faster — it
just starves the reapers and piles processes up. So `Max` is the fastest profile
*and* keeps the live population to a few hundred (no pile-up). The default is
**Balanced** — fast, with headroom, and easy on the laptop. The tier
(`ResourceProfile`) lives in `rusm-node` `profile.rs`; the benchmark's spawn-worker
tuning for each tier is in `rusm-bench` `profile_tuning.rs`.

## Protocol

The node and clients speak a small JSON protocol (`rusm-bench` `protocol.rs`,
mirrored in `bench/dashboard/src/types.ts`):

- Server → client: `hello { scenarios, profiles }`, `tick { frame }`, `error { message }`.
- Client → server: `run { scenario }`, `stop`, `set_observer_detail { enabled }`,
  `set_resource_profile { profile }`.

A `Frame` carries the scenario, running flag, throughput, latency snapshot,
observer snapshot, and the **active resource profile**. The dashboard folds
messages into state with a pure reducer (`state.ts`) — fully unit-tested.
