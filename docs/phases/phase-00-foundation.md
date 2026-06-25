# Phase 0 — Observability foundation

> **The Phase log is RUSM's build history.** These pages tell how the runtime was built, phase by phase — the why, the design, and what each phase shipped — not how to *use* a feature today. For that, start with [Build an app](/build-an-app/url-shortener); for the whole arc at a glance, see the [roadmap](/about/roadmap).

You can't improve what you can't measure — Phase 0 built the benchmark harness, live observer, and dashboard *before a single line of runtime existed*, so every subsequent phase arrived to a waiting feedback loop.

## Why this phase

Every later phase needs a feedback loop: implement real spawning and immediately see the number. Without an observability layer in place from day one, each phase would require retrofitting metrics, wiring a UI, and hoping the numbers meant something. Built last, the harness would always be playing catch-up.

The solution: build the measurement infrastructure first. Every scenario starts as synthetic data — a pure deterministic function of `(scenario, tick)` — and **graduates to real data** the moment the underlying phase completes. Phase 0 is why every later phase has a live dashboard tile waiting for it on arrival.

## What shipped

1. **`rusm-metrics`** — `Counter` (relaxed atomic), `LatencyHistogram` (HdrHistogram-backed p50/p95/p99), `TimeSeries` (ring buffer). Zero per-operation overhead on the hot path.
2. **`rusm-observer`** — `Observer` folds aggregate counters + a live process slice into an `ObserverSnapshot`, with a detail on/off toggle so the per-instance table is opt-in.
3. **`rusm-bench`** — scenario menu, a deterministic `SyntheticSource`, a clock-free `Runner` that aggregates ticks into `Frame`s, the JSON wire protocol, and a Tokio + tokio-tungstenite WebSocket server. A real WebSocket client integration test drives the whole stack end to end.
4. **`rusm-cli`** — `rusm node start` and the `rusm attach` REPL for connecting to a live node.
5. **Dashboard** (`bench/dashboard`, React + Bun + uPlot) — benchmark view + live observer. Pure logic (`format`, `protocol`, `state`) is fully unit-tested; the React layer is presentational.
6. **Embedded node example** — [`embedded_node`](https://github.com/archan937/rusm/tree/main/examples/embedding/embedded_node) shows how to embed a node in a host process.

## Design highlights

- **Synthetic → real graduation model.** Each scenario starts as a deterministic preview. When the real engine lands, the same slot receives live data — no new wiring needed. The UI is always ready.
- **Relaxed-atomic counters, never per-event hooks.** Counters increment with relaxed atomics; a periodic snapshot folds them into the observer. No locking, no tracing overhead on the hot path.
- **Node/client separation established early.** The dashboard and REPL are clients of a node's control channel over WebSocket — the same architectural pattern that becomes [live attach](/deep-dive/live-attach) for production nodes in Phase 9.
- **Deterministic synthetic data for stable tests.** A pure `(scenario, tick)` function makes the dashboard lively in development and integration tests stable — no timers, no randomness needed.

## What this unlocks

With Phase 0 in place, every future phase has an instant feedback loop: land the feature, run the bench, watch the scenario tile flip from synthetic to live numbers. There is no phase that ships without visible proof.

It also establishes the node model — a `rusm node start` process that clients attach to — which becomes the foundation for the `rusm attach` REPL, the distributed dashboard, and live process inspection in later phases.

## Try it

```sh
cargo run -p rusm-bench -- start                  # start the benchmark node + dashboard WebSocket server
cargo run -p rusm-cli -- attach                   # connect the REPL; try: detail on / detail off
cd bench/dashboard && bun install && bun run dev  # open the live dashboard in a browser
cargo run -p rusm-bench -- run spawn-storm 5      # 5 seconds — synthetic here, live after Phase 1
```

## Status

Phase complete. All 21 dashboard scenarios run on real data as of Phase 9. Dashboard logic covered at 100%; workspace coverage ≥98%.

---

*Next: [Phase 1](./phase-01-process-core.md) — the process & scheduler core: a process becomes a real Tokio task with an abort-based lifecycle, and spawn-storm goes live at ~2.4M spawns/sec.*
