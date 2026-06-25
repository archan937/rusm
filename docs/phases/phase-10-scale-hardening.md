# Phase 10 — Scale & hardening

Phase 10 is not about speed — RUSM already runs at the isolation-model ceiling. It's about surviving scale, overload, and attack: lift the fixed instance cap, protect against memory exhaustion, secure the cluster properly, and stop crash loops before they cascade.

## Why this phase

~440k component spawns/sec and ~21M msgs/sec are already at the hardware ceiling for this isolation model. What breaks in production is not raw throughput — it's the edge cases. A pool that fills up and blocks. A fast producer that grows a slow consumer's mailbox without bound. A compromised node that can re-join the cluster with a stolen cert. A supervisor that restarts a crashing child forever and runs out of resources.

Phase 10 closes those four failure modes, one by one, without touching the happy-path numbers.

## What shipped

1. **On-demand instance tier** — `WasmRuntime::with_overflow` adds a second, on-demand engine behind the pooling allocator. A spawn reserves a pooled slot via an atomic counter — exactly `cap` claims can be outstanding at once. Once the pool is full, the spawn instantiates on the on-demand engine instead. The live Wasm-process count is now bounded by **available memory**, not a fixed pool size. Without this, spawns past a full pool blocked indefinitely.
2. **Opt-in bounded mailboxes** — `Runtime::with_mailbox_capacity(n)` sheds *user* messages once a mailbox holds `n`, so a fast producer can't grow a slow consumer's memory without bound. **System signals are never shed** — exits and monitor-downs bypass the capacity check, so back-pressure never breaks links, monitors, or supervision trees. The default (unbounded) path is untouched — one predicted branch, no new atomics.
3. **Mutual TLS with per-node certs** — `ClusterCa::generate()` + `ca.issue(node)` give each node its own keypair and a CA-signed certificate. Every cluster connection is mutually authenticated: the server requires a client cert, both sides verify against the trust anchor. A node from a foreign CA is rejected at the handshake. A compromised node can be excluded by rotating the CA without re-keying the rest of the cluster. Cost is handshake-only; steady-state throughput is unchanged.
4. **Supervisor restart-intensity** — both rusm-rs and rusm-ts now implement Erlang's `{max_restarts, max_seconds}`: `Supervisor::within(Duration)` / `supervise({ maxRestarts, maxSeconds })` give up only if more than `max_restarts` happen *within a sliding window* — not a lifetime counter, which wrongly penalized long uptime and gave no crash-loop protection. A burst trips the intensity limit and the supervisor exits, letting failure escalate. Occasional crashes spread over hours never accumulate.

## Design highlights

- **Overflow `InstancePre` without recompilation.** The overflow engine shares the same component bytes via serialization/deserialization — no second compile step, no new source to maintain. The epoch ticker drives both engines so overflow guests are preempted identically to pooled guests.
- **Bounded mailboxes with zero hot-path cost.** The default unbounded path sees one predicted branch. Enabling bounded mailboxes adds a single check on *user* messages only. The system signal path is entirely separate — a bounded mailbox can never prevent a supervision signal from arriving.
- **Sliding-window intensity over lifetime counters.** A service that runs for three days and has three crashes spread across that time should not trigger a restart-intensity limit. Only a burst — more than N crashes in M seconds — indicates a crash loop. The old lifetime counter got this wrong; the sliding window gets it right.
- **No regression on the hot paths.** Before and after measurement: component-storm holds ~430–440k spawns/sec; ping-pong holds ~21M msgs/sec; cross-node throughput is unchanged (mTLS costs only at handshake). A first draft of the overflow tier double-cloned `InstancePre` and dropped component-storm to ~415k; fixed by moving the chosen pre instead of cloning.

## What this unlocks

RUSM can now run unbounded workloads without hitting a fixed instance cap. A bounded mailbox protects any slow-consumer component from being overwhelmed. The cluster is properly secured — each node is individually authenticated, and a key rotation excludes a bad actor without disrupting the rest. Supervisors correctly distinguish crash loops from occasional failures.

Together, these four changes take RUSM from "fast and correct on a well-behaved workload" to "durable under real-world adversarial conditions."

## Try it

```sh
cargo run --release -p rusm-bench -- run component-storm 5   # overflow tier active; ~440k holds
cargo run --release -p rusm-bench -- run ping-pong 5         # bounded mailbox path; ~21M holds
cargo test -p rusm-cluster tls                               # mTLS + foreign-CA rejection
```

## Status

Phase complete. All regressions held. Component-storm ~440k spawns/sec, ping-pong ~21M msgs/sec, cross-node throughput unchanged. Deferred to Phase 11: native p3-typed `stream<u8>` WIT signature (handle-based byte streams are fully functional).

---

*Next: [Phase 11 — Serving & the standard-WASI surface](/phases/phase-11-serving) — HTTP, WebSocket, and SSE serving from any component in any language, declarative routing, `wasi:cli/run` support, and the full three-language serving benchmark suite.*
