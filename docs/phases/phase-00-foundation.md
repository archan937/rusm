# Phase 0 — observability foundation

**Goal:** before any runtime internals, build the thing we measure everything
with — a benchmark + live-observer dashboard — and prove it works end to end on
synthetic data. Every later phase graduates a scenario from synthetic to real.

## Why first

You can't improve what you can't see. By building the harness, protocol, and
dashboard up front, every subsequent phase has an immediate, visual feedback
loop: implement real spawning → the spawn-storm scenario lights up with real
numbers, with no new plumbing.

## What we built (TDD throughout)

1. **`rusm-metrics`** — `Counter` (relaxed atomic), `LatencyHistogram`
   (HdrHistogram-backed p50/p95/p99), `TimeSeries` (ring buffer). 100% covered.
2. **`rusm-observer`** — `Observer` folds aggregate counters + a process slice
   into an `ObserverSnapshot`, with a **detail on/off** toggle so the per-instance
   table can be skipped. 100% covered.
3. **`rusm-bench`** — `Scenario` menu, a deterministic `SyntheticSource`, a
   clock-free `Runner` that aggregates ticks into `Frame`s, the JSON wire
   `protocol`, and a Tokio + tokio-tungstenite **WebSocket server** (`Node` +
   `serve`). A real WebSocket client integration test drives the whole thing.
4. **`rusm-cli`** — `rusm node start` and the `rusm attach` REPL.
5. **Dashboard** (`bench/dashboard`, React on Bun, uPlot) — benchmark view + live
   observer; pure logic (`format`, `protocol`, `state`) unit-tested to 100%.
6. **Harness & examples** — the benchmark runner, the deterministic synthetic source,
   and the observer detail-toggle (all in `bench/rusm-bench`); the runnable
   [`embedded_node`](https://github.com/archan937/rusm/tree/main/examples/embedding/embedded_node) example embeds a node.

## Concepts introduced

- **Low-overhead observability** — relaxed-atomic counters + periodic snapshots,
  never an event per op. See [03-benchmark-dashboard](../03-benchmark-dashboard.md).
- **Deterministic synthetic data** — pure function of `(scenario, tick)`, so the
  dashboard is lively and tests are stable.
- **A node and its clients** — the dashboard and REPL are clients of a node's
  control channel; the same shape becomes [live attach](../concepts/live-attach.md).

## Play with it

```sh
cargo run -p rusm-bench -- run spawn-storm 5          # real spawns, sampled ticks
cargo run -p rusm-bench -- start                      # the dashboard node (or: make dashboard)
cargo run -p rusm-cli -- attach                       # then `detail off` — observer on vs off
```

## Verification

`cargo test` (all crates + the live-server integration test) green; workspace
coverage ≥98% (mostly 100%); dashboard `bun test --coverage` 100% on logic;
`cargo fmt` + Prettier clean.

## Next

[Phase 1](./phase-01-process-core.md): the **process & scheduler core** — a process =
Tokio task + process table + abort-based lifecycle, running a native Rust body
(the mailbox arrives with messaging in Phase 2). The first real step toward the
actor model; Wasmtime arrives as the *backend* in Phase 6.
