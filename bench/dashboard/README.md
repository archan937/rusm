# RUSM benchmark dashboard

A live **benchmark + observer dashboard** for RUSM — a Bun/Vite **React** app that streams a
running node's metrics (latency, throughput, peak concurrency) and its live process table to
**uPlot** charts, with an observer-on/observer-off toggle to show that observability is nearly
free. It's the visual half of the story told in
[about/benchmark-dashboard-and-observer](../../docs/about/benchmark-dashboard-and-observer.md).

## Run it

The dashboard reads from a running node's attach endpoint, so start one first, then the UI:

```sh
# 1. a node that runs the benchmark scenarios (repo-only tool) — exposes ws://127.0.0.1:4000
cargo run -p rusm-bench -- start

# 2. the dashboard (Bun — never Node.js)
cd bench/dashboard
bun install
bun run dev            # http://localhost:5173, connects to the node above
```

Any RUSM node works as a source — `rusm node start` (or the `embedded_node` example) exposes
the same attach endpoint, and `rusm attach` is the terminal equivalent of this UI.

## Develop

```sh
bun run build          # production build (vite)
bun test               # tests
bun test --coverage    # coverage (the dashboard's half of the project's ~100% bar)
bun run fmt            # prettier
```

Stack: **Bun** + **Vite** + **React** + **uPlot**. Presentational `.tsx` is excluded from the
coverage bar; the data/transform layer is tested.
