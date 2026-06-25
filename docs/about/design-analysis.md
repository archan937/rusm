# Design analysis

A runtime is only as trustworthy as it is honest about itself. This page is RUSM's
self-assessment, kept current as the project evolves: what the design genuinely gets
right, how it performs, where the real opportunities are, and the known lackings —
each with its status, so nothing is quietly swept aside. The "lackings" table below
is the part to read first if you're sceptical; most of what it once tracked has since
shipped, and the rows that haven't are stated plainly.

## Where it's superior

- **A Wasm-free OTP core.** `rusm-otp` (processes, mailboxes, links/monitors/
  supervision, registry, timers, TCP) has *zero* Wasmtime dependency — enforced by
  the dependency graph. The actor model is a standalone Rust library; Wasmtime is a
  swappable backend. Reusable, testable in isolation, uncontaminated.
- **Cheaper processes than Lunatic.** One channel per process (exit signals ride
  the mailbox; kill is an abort-handle flag-flip) vs Lunatic's two channels.
- **A component-model host — an axis Lunatic lacks.** Core modules **+** components
  (WASI p2/p3) **+** TS/JS, all instance-per-process actors, with a WIT actor world
  callable from any language. Composition is **message passing, not a lattice**.
- **No execution-time cap** (vs wasmCloud's 30s); long-lived supervised actors.
  Epoch preemption on a **dedicated OS thread** (preemption that can't be starved).
- **Default-deny capabilities per process** — now including the actor process-
  control surface (see #2 below).

## Performance

- ~2.4M native spawns/s, ~440–475k Wasm spawns/s, ~21M msgs/s (p50 <1µs), fairness
  50M→400M+ ops/s, **15+ GB/s** cross-process streaming. Pooling + CoW +
  `InstancePre` + precomputed-export-index make instance-per-process cheap; Tokio's
  work-stealing scheduler + mpsc do the heavy lifting (battle-proven, not reinvented).
- The native→Wasm ~5× gap is the memory-isolation tax, paid once. Streaming's two
  copies are irreducible across isolation boundaries.
- Most numbers are in-process/loopback — they prove the runtime isn't the
  bottleneck, not network throughput.

## Opportunities

Most of what this section once tracked has since shipped:

- **HTTP(S)/WS(S)/SSE serving via `wasi:http`** — Phase 11 (it also unlocked `fetch`
  in TS guests: a Tokio HTTP client + fiber suspension).
- **Distributed cluster** — Phase 9 (`rusm-cluster`, QUIC+TLS): single-node → horizontal.
- **On-demand instance tier above the pool** — Phase 10.
- **Guest crates** (`rusm-rs`/`rusm-ts`/`rusm-go`, service macros, typed clients, in-guest
  `Supervisor` strategies) — Phase 8.

The one genuinely open opportunity is **a true head-to-head benchmark vs Lunatic**: the
architectural case is made (pooling + CoW + epoch vs on-demand + fuel); what's missing is
numbers from both runtimes on the same box.

## Lackings — status

| # | Lacking | Status |
| --- | --- | --- |
| 1 | Wasm-instance concurrency ceiling | **Solved (Phase 10)** — configurable via `WasmRuntime::with_limits` (default raised 256→1024, lazy virtual reservation), plus an **on-demand overflow tier** (`WasmRuntime::with_overflow`) above the pool: past the pooled cap, spawns come from an on-demand engine, so the live Wasm-process count is bounded by **available memory**, not a compile-time size. |
| 2 | Actor ABI not capability-scoped (untrusted code could kill/enumerate any process) | **Solved** — default-deny `allow_process_control`; a sandboxed guest manages only itself. Enforced on both bridges, gate-tested. |
| 3 | Unbounded mailboxes (a fast producer can grow a slow consumer's mailbox) | **Solved (Phase 10, opt-in)** — `Runtime::with_mailbox_capacity` sheds *user* messages past capacity (system/exit signals are never dropped), reusing the opt-in depth counter. Unbounded stays the Erlang-compatible default; bounding the *serve* path by default is the remaining Phase 12 item. |
| 4 | Shallow supervision (links/monitors/restart-bool, not OTP strategies) | **Solved (in-guest)** — Phase 8 ships an in-guest `Supervisor` (`one_for_one`/`one_for_all`/`rest_for_one` + `max_restarts`) over a `monitor` ABI, in both `rusm-rs` and `rusm-ts`. |
| 5 | DX/toolchain friction | **Largely a non-issue** — a TS dev needs only Bun (the `rusm-ts` npm package + `rusm dev` watch/reload); wasi-sdk is a one-time *maintainer* build dep (the runner is prebuilt). `rusm new <name>` scaffolds a ready-to-serve app in one command. |
| 6 | TS guests lacked Web APIs | **Solved** — full Web API polyfills (`bridge/webapi.js`: TextEncoder/URL/Headers/ReadableStream/…), transparent to the dev. `fetch` awaits `wasi:http` (the one genuinely network-bound API). |
| 7 | Selective receive is O(n) over the save queue | **Accepted** — inherent to selective-receive semantics (so is the BEAM's); the common `recv` path is O(1). |
| 8 | Distribution is roadmap; `distributed-fanout` is synthetic | **Solved (Phase 9)** — the Wasm-free `rusm-cluster` QUIC+TLS transport: cross-node send, gossiped global registry, remote spawn, live attach (~550k cross-node msgs/s). The `distributed-fanout` dashboard scenario now runs on this **real** engine — no synthetic scenarios remain. |

## Verdict

The core architecture — the Wasm-free OTP boundary, the component-model host with
message-passing composition, the no-time-cap lifetime model, and a **capability-scoped**
actor ABI — is differentiated and sound. The two items once flagged highest-priority (the
**on-demand instance tier** #1 and **bounded mailboxes** #3) have since shipped in Phase
10. What remains is operational edge-hardening (**Phase 12**): HTTP request-body/timeout
admission control, default-bounded *serve-path* mailboxes, and authenticated cluster
gossip — network-edge and peer-trust gaps, not architectural ones.
