# Design analysis

An honest assessment of RUSM's design — what's superior, how it performs, where
the opportunities are, and the known lackings (with their current status). Updated
as lackings are closed.

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

1. **HTTP(S)/WS(S)/SSE serving via `wasi:http`** — pairs with the GB/s streaming;
   also unlocks `fetch` in TS guests (Tokio HTTP client + fiber suspension).
2. **Distributed cluster (Phase 9, QUIC+TLS)** — single-node → horizontal.
3. **A true head-to-head benchmark vs Lunatic.**
4. **On-demand instance tier** above the pool (see #1).

(Phase 8 — the `rusm-rs`/`rusm-ts` guest crates, service macros, typed clients, and
in-guest `Supervisor` strategies — is shipped.)

## Lackings — status

| # | Lacking | Status |
| --- | --- | --- |
| 1 | Wasm-instance concurrency ceiling | **Mitigated** — configurable via `WasmRuntime::with_limits`; default raised 256→1024 (lazy virtual reservation). A true "millions" tier needs an on-demand fallback above the pool — **roadmap (Phase 10)**. |
| 2 | Actor ABI not capability-scoped (untrusted code could kill/enumerate any process) | **Solved** — default-deny `allow_process_control`; a sandboxed guest manages only itself. Enforced on both bridges, gate-tested. |
| 3 | Unbounded mailboxes (a fast producer can grow a slow consumer's mailbox) | **Roadmap** — opt-in bounded mailbox (load-shed/back-pressure), reusing the opt-in depth counter. Erlang has the same default. |
| 4 | Shallow supervision (links/monitors/restart-bool, not OTP strategies) | **Solved (in-guest)** — Phase 8 ships an in-guest `Supervisor` (`one_for_one`/`one_for_all`/`rest_for_one` + `max_restarts`) over a `monitor` ABI, in both `rusm-rs` and `rusm-ts`. |
| 5 | DX/toolchain friction | **Largely a non-issue** — a TS dev needs only Bun (the `rusm-ts` npm package + `rusm dev` watch/reload); wasi-sdk is a one-time *maintainer* build dep (the runner is prebuilt). `rusm new <name>` scaffolds a ready-to-serve app in one command. |
| 6 | TS guests lacked Web APIs | **Solved** — full Web API polyfills (`bridge/webapi.js`: TextEncoder/URL/Headers/ReadableStream/…), transparent to the dev. `fetch` awaits `wasi:http` (the one genuinely network-bound API). |
| 7 | Selective receive is O(n) over the save queue | **Accepted** — inherent to selective-receive semantics (so is the BEAM's); the common `recv` path is O(1). |
| 8 | Distribution is roadmap; `distributed-fanout` is synthetic | **Solved (Phase 9)** — the Wasm-free `rusm-cluster` QUIC+TLS transport: cross-node send, gossiped global registry, remote spawn, live attach (~550k cross-node msgs/s). The `distributed-fanout` dashboard scenario now runs on this **real** engine — no synthetic scenarios remain. |

## Verdict

The core architecture — the Wasm-free OTP boundary, the component-model host with
message-passing composition, the no-time-cap lifetime model, and now a
**capability-scoped** actor ABI — is differentiated and sound. The two highest-
priority remaining items are the **on-demand instance tier** (#1) and **bounded
mailboxes** (#3); neither is an architectural dead-end.
