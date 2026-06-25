# Phase 6 — Wasmtime as the process backend

Phase 6 proves the core bet: because the OTP layer was built Wasm-ready from the start, adding Wasmtime is *additive* — the same `spawn()` call, now running a sandboxed Wasm instance with true memory isolation.

## Why this phase

Phases 1–5 built a real, fast, measurable actor model on native Rust bodies. Phase 6 is the answer to the question those phases were designed to make possible: can we swap the native body for a Wasm instance without touching the actor model?

The answer is yes — because `rusm-otp` was always designed with a hard boundary. All Wasmtime lives in `rusm-wasm`; `rusm-otp` has zero `wasmtime` dependency. The dependency graph enforces it. Phase 6 proves the bet pays off: a process is still a Tokio task and a table entry. Now its body is a guest instance that can crash, loop, or misbehave without touching anything outside its sandbox.

## What shipped

1. **`WasmRuntime`** over a shared `rusm-otp` `Runtime` — owns the Wasmtime `Engine`, a `Linker<Host>`, and shared `Counters`. Thin wrapper; the actor core is unchanged.
2. **Instance-per-process** — `compile(wat) -> Module`, `prepare(module) -> InstancePre<Host>`, `spawn(prepared, entry) -> ProcessHandle`. Each spawn instantiates a fresh, isolated Wasm instance as a `rusm-otp` process.
3. **Three fast-spawn levers on one `Engine`** — all working together:
   - **Pooling allocator** — instances, memories, and tables recycled from a pool.
   - **`memory_init_cow`** — copy-on-write memory images; a fresh instance doesn't zero or copy its whole linear memory.
   - **Per-module `InstancePre`** — type-checking and host-import resolution done once at `prepare`, not per spawn.
4. **Epoch-interruption preemption** — even a guest running `loop { }` is forced to yield and stays killable. The epoch bumper runs on a **dedicated OS thread** — not a Tokio task. This is critical: a Tokio task could be starved by the very guests it must preempt, deadlocking. The store yields async on each epoch tick.
5. **Host ABI via `Caller::data`** — `rusm::self_pid` (the guest's own pid) and `rusm::notify` (bumps a shared counter) — the seed of the [host ABI](/deep-dive/host-abi-reference) fully delivered in Phase 7.
6. **Trap → `ExitReason::Crashed`** — a guest trap reports through the same exit machinery as a native crash from Phase 3. Links, monitors, and supervisors see no difference.
7. **Fairness engine** (`rusm-bench`) — Wasm spinners saturate every core while Wasm bystanders keep calling `notify`. A nonzero bystander rate (~50M+ ops/sec under load, past 400M on free cores) *is* the proof that epoch preemption works.

## Design highlights

- **Dedicated epoch thread — the single most important correctness decision in this phase.** A preemption mechanism that can itself be preempted is no mechanism at all. The OS-level thread runs regardless of Tokio scheduler pressure.
- **Three spawn levers cost almost nothing individually; together they compound.** Pooling eliminates allocation; CoW eliminates zero-copy; `InstancePre` eliminates per-spawn type resolution. The combination pushes spawn cost down to nanoseconds.
- **The Wasm-free boundary is machine-enforced.** `cargo tree -p rusm-otp` shows no `wasmtime` anywhere in the graph. This isn't a convention — it's a compile-time guarantee.
- **The bench counts honestly.** It asserts `notifications == n` (every guest actually ran its body), so crashed instances can't inflate the spawn rate number.

## What this unlocks

The entire OTP actor model — spawn, kill, link, monitor, supervise, registry, timers — now applies to Wasm instances without modification. A Wasm guest that panics becomes `ExitReason::Crashed` and propagates through links. A supervisor can restart a crashed guest the same way it restarts a crashed native process.

Phase 7 delivers the modern artifact (WASM components instead of core modules) and the full `rusm:runtime` WIT actor world. The optimized component path will reach ~440k spawns/sec — the foundation is here.

## Try it

```sh
cargo run -p rusm-bench -- run fairness 5     # spinners saturate all cores; bystanders still run
cargo test -p rusm-wasm                       # instance-per-process, traps, epoch preemption
```

## Status

Phase complete. Fairness is live in the dashboard. The Wasm-free invariant holds. The spawn optimizations here are the foundation for ~440k component spawns/sec in Phase 7.

---

*Next: [Phase 7](./phase-07-components.md) — component hosting: the WASM component model, the `rusm:runtime` WIT actor world, and ~440k component spawns/sec.*
