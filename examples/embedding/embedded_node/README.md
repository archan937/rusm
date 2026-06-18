# embedded_node

Embed a RUSM benchmark **node** in your own program and serve the live protocol,
then drive it from the dashboard or a REPL. This is how you'd run a node from
inside an application rather than via the `rusm` CLI.

## What it shows

- Building a `Node` with `Node::new`.
- Serving the control/observer WebSocket with `serve(addr, node)`.
- That the dashboard and `rusm attach` are just clients of that channel.

## Run

```sh
cargo run -p rusm-bench --example embedded_node
```

You'll see:

```
RUSM node on ws://127.0.0.1:4000 — attach the dashboard or `rusm attach`
```

The process then serves until you stop it (Ctrl-C).

## Connect to it — the REPL

In another terminal, attach a live REPL (like `iex --remsh`). No URL is needed for
the local node; you can also pass `host`, `host:port`, or a full `ws://` URL:

```sh
cargo run -p rusm-cli -- attach
```

On connect you'll see the prompt line and the scenario menu the node sends. Then
type commands (`help`, `run <scenario>`, `stop`, `detail on|off`, `quit`). A
`run` streams one summary line per tick (~20/sec) until you `stop`:

```text
attached to ws://127.0.0.1:4000 — type `help` for commands
connected. scenarios:
  spawn-storm          Spawn storm
  ping-pong            Message ping-pong
  fairness             Fairness under tight loop
  fault-recovery       Fault recovery
  connection-storm     Connection storm (300k/s proof)
  distributed-fanout   Distributed fan-out
run connection-storm
[connection-storm]       324559 ops/s  peak    4552  p50     284µs  p99     498µs  procs 4552
[connection-storm]       332552 ops/s  peak    4552  p50     275µs  p99     497µs  procs 2276
stop
quit
```

(Throughput/latency numbers vary per tick within the scenario's synthetic ranges.)

## Connect to it — the dashboard

```sh
cd bench/dashboard && bun install && bun run dev   # then open the printed URL
```

Pick a scenario from the menu, press **Run**, and watch the live throughput
chart, stat cards, and the host/instance observer update in real time.
