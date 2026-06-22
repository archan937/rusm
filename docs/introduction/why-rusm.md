# Why RUSM?

RUSM gives you the BEAM's process model — cheap isolated processes, "let it crash"
supervision, built-in distribution, live introspection — but for **any language**,
compiled to **WebAssembly**, on a Rust + Tokio runtime that is lightweight and *crazy
fast*. Elixir's concurrency and fault tolerance, in Rust, running Wasm.

## The itch

My Elixir years left me wanting one thing: the BEAM's process model, but able to run
**any** language on infrastructure that is lightweight, optimal, and fast.
[Lunatic](https://github.com/lunatic-solutions/lunatic) proved it was possible and
pitched it perfectly — then it went quiet. **RUSM exists to carry that torch forward.**
If Lunatic were still active and current, I'd just use it.

## What a process should be

RUSM is built around a single idea: **a process is an isolated WebAssembly instance** —
its own stack, heap, syscalls, and permissions. Almost everything else falls out of that
one decision:

- **Crash isolation** — one process can never corrupt another; a guest trap crashes only
  *that* process, never its neighbours or the node.
- **Massive, cheap concurrency** — processes are Tokio tasks multiplexed M:N over a few OS
  threads, targeting hundreds of thousands of spawns per second (~2.4M/sec measured).
- **Write blocking code, get async** — Wasmtime fibers suspend a guest's "blocking" call
  while the host awaits; guests never see `async`.
- **Survivable failure** — links, monitors, and supervisors, Erlang-style.
- **First-class clusters** — nodes connect over QUIC + mutual TLS; processes spawn and
  message across nodes, and you can **attach a live REPL/observer to a running node**
  (like `iex --remsh`).

## Proof, not promises

RUSM ships with a **benchmark + live-observer dashboard** that stress-tests the runtime and
streams real latency, throughput, peak concurrency, and a live process table — with an
observer-on / observer-off toggle to show that observability is nearly free. Network-facing
rates (HTTP/WS/SSE throughput, connection establishment) are **earned out-of-process** by
the `rusm-loadtest` driver against a live `rusm serve` port — e.g. ~34k
sandboxed-process-per-connection WS establishments/sec — measured, never asserted.

It was built in small, test-driven [phases](/about/roadmap), each one teaching a single
concept: the Wasm-free OTP core first (processes, messaging, supervision, management, TCP),
then Wasmtime as the process backend, then real component hosting, clustering over QUIC+TLS,
and serving. Every dashboard scenario now runs on real data.

Next: see [what you get](/introduction/what-you-get) for the full feature map, or go
straight to the [quick start](/introduction/quick-start).
