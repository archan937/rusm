# Live attach

One of the BEAM's best tricks: connect to an *already-running* node and inspect or
poke it live — `iex --remsh`, or `:observer` attached to production. RUSM brings
this to WebAssembly.

## What it is

An attachable node exposes a live **observe channel** over WebSocket. A client
that attaches today can:

- watch the live process count and a per-process table (label, registry names,
  mailbox depth, links) stream in;
- toggle that detail table (`detail on|off`);
- **evaluate JavaScript against the live node** — a stateful shell (bindings persist
  across lines) with the full `Process` API, so you can `whereis` / `list` / `send` /
  `kill` processes and even `connect()` to a service live. Gated to **loopback** clients
  until the channel is authenticated; see [the live REPL](/deep-dive/the-live-repl);
- (later) hot-reload a module; spawn-through and remote eval over an authenticated channel.

`rusm node start` serves this channel; the **`rusm attach <node>`** REPL renders
it in the terminal. The **benchmark dashboard** is a *separate* node (`rusm-bench
start`, repo-only) with its own richer channel — the scenario-driving
observer GUI behind [the dashboard](/about/benchmark-dashboard-and-observer).

## This is new — Rust doesn't give it to us

The BEAM bakes remote shell + distribution + observer into the VM. Rust has no
runtime VM, no built-in process/node model, and no live introspection, so RUSM
builds this itself on the control channel. The closest prior art,
[`tokio-console`](https://github.com/tokio-rs/console), is read-mostly
diagnostics over a gRPC endpoint — useful inspiration, but not a process-aware
REPL you can spawn and message through.

## Where it stands (Phase 9)

Two layers are real:

- **Local channel (Phase 0+):** `rusm node start` serves an observe channel over
  WebSocket; `rusm attach` connects to it to watch live process samples and toggle
  the detail table. (The benchmark dashboard runs the same idea over its own node,
  `rusm-bench start`, with scenario controls layered on.)
- **Cross-node primitive (Phase 9):** over the cluster transport, a node's
  control-plane RPC answers `remote_pids(node)` — list the processes alive on a
  *peer*. That's the building block behind attaching to a remote node; richer
  remote introspection (per-process info, message/spawn-through) extends the same
  RPC.

> The transport, control channel, and remote listing are real; the full remote
> `rusm attach <node>` REPL surface builds on this primitive.
