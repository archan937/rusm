# Phase 10 — scale & hardening

**Goal:** not raw speed — RUSM's throughput and latency are already at the
isolation-model ceiling (~440k component spawns/s, ~21M msgs/s). Phase 10 is about
surviving **scale, overload, and attack**: lift the fixed instance cap, protect
against overload, secure the cluster properly, and stop crash loops. No new
user-facing features — production hardening of what's already there.

## What we built (TDD throughout)

1. **On-demand instance tier** (`rusm-wasm`). `WasmRuntime::with_overflow` adds a
   second, on-demand engine behind the pooling allocator. A spawn **reserves a
   pooled slot** via an atomic counter — exactly `cap` claims can be outstanding, so
   a claimed pooled spawn never blocks on exhaustion — and once the pool is full it
   instantiates on the on-demand engine instead. The live *Wasm*-process count is
   then bounded by **available memory**, not a compile-time pool size. The overflow
   `InstancePre` is prepared without recompiling (serialize the compiled component,
   deserialize into the overflow engine), and the epoch ticker drives *both* engines
   so overflow guests are preempted too. Found along the way: without overflow,
   spawns past a full pool **block indefinitely** — exactly what this fixes.
2. **Opt-in bounded mailboxes** (`rusm-otp`). `Runtime::with_mailbox_capacity(n)`
   sheds *user* messages once a mailbox holds `n`, so a fast producer can't grow a
   slow consumer's memory without bound. **System signals are never shed** — exits
   and monitor-downs ride the same single mailbox but bypass the capacity check, so
   back-pressure never breaks links, monitors, or supervision. The default
   (unbounded) path is untouched — one predicted branch, no new atomics.
3. **Cluster security hardening** (`rusm-cluster`). `ClusterCa::generate()` +
   `ca.issue(node)` give each node its **own** keypair and a CA-signed certificate;
   every link is **mutually authenticated** (server requires a client cert, both
   verify against the trust anchor). A node from a foreign CA is rejected at the
   handshake. This replaces Phase 9's single pre-shared cluster cert, so a
   compromised node can be excluded by rotating the CA without re-keying the rest.
   Cost is handshake-only; steady-state throughput is unchanged.
4. **Supervisor restart-intensity** (both guests). Erlang's
   `{max_restarts, max_seconds}`: `rusm-rs`'s `Supervisor::within(Duration)` and
   `rusm-ts`'s `supervise({ maxRestarts, maxSeconds })` give up only if more than
   `max_restarts` happen **within a sliding window** — instead of the old lifetime
   counter, which wrongly penalised long uptime and gave no crash-loop escalation.
   A burst trips it (the supervisor exits, letting failure escalate); occasional
   crashes spread over time never accumulate.

## No regression

The hot paths were held to their numbers, measured before/after:

- **component-storm** ~430–440k spawns/s (a first draft of the overflow tier
  double-cloned the `InstancePre` and dropped it to ~415k; fixed by moving the
  chosen pre instead of cloning it);
- **ping-pong** ~21M msgs/s, **spawn-storm** ~2.48M spawns/s (bounded mailboxes add
  nothing to the default unbounded path);
- cross-node throughput unchanged (mutual TLS costs only at the handshake).

## Verification

`cargo test` green across the workspace. New tests: overflow spawns past a pool of
2 (five long-lived instances all come alive); a bounded mailbox sheds user messages
past capacity while a full mailbox still delivers a system `Down`; same-CA nodes
form a cluster and a foreign-CA node is rejected; and a rapid kill-burst makes both
the Rust **and** TS supervisors give up past their restart intensity. `cargo fmt` +
clippy clean.

## Next

[Phase 11](/about/roadmap): the **standard-WASI surface** — `wasi:http` hosting
(serve HTTP/WS/SSE from a component), the `wasi:cli/run` entrypoint, and a native
p3-typed `stream<u8>` for the actor world.
