# How RUSM compares — hydra, Lunatic & wasmCloud

*A runtime, a library, and a platform — so this is a cross-category comparison, not a
like-for-like one (see [They are different categories](#they-are-different-categories)).*

> **How to read this.** This is a *structured opinion*, not a benchmark. The scores
> are a deliberately-coarse way to compare design choices across four projects that
> are **not really the same kind of thing**; they are not lab measurements. Where a
> claim rests on hands-on experience it is **attributed and version-qualified** —
> these projects evolve, and a thing that was true of one release may not hold for
> the next. RUSM is stated honestly throughout as **young and unproven at scale**:
> it addresses several problems *by design*, which is not the same as having earned
> trust in production. Read the trade-offs and draw your own conclusion.

## They are different categories

A fair comparison starts by admitting the systems solve overlapping but distinct
problems:

- **RUSM** — an Erlang/OTP-style core (Wasm-free) that runs **WebAssembly components**
  as long-lived, supervised, capability-sandboxed processes, with HTTP/WS/SSE serving
  and a QUIC/mTLS cluster. A **runtime**.
- **hydra** — a native-Rust OTP implementation (GenServer, Supervisor, links/monitors),
  **no WebAssembly**. A **library**.
- **Lunatic** — a WebAssembly actor **runtime** (processes are Wasm instances), the
  closest peer to RUSM in spirit.
- **wasmCloud** — a distributed WebAssembly **application platform**: components wired
  to capability providers over a NATS "lattice", with declarative deployment.

Because they are different categories, no single weighting is "correct". The matrix
below weights an actor/process model, supervision, isolation, and reliability
heavily, because those are RUSM's design priorities — a reader optimising for, say,
turnkey multi-cloud operations should re-weight accordingly.

## What each one is (a fair sketch)

- **RUSM** owns process lifecycles end-to-end (the Wasm-free core), runs the WASM
  **component model** (WASI p2/p3) with default-deny per-process capabilities, serves
  HTTP/WS/SSE from RS *and* TS guests, and clusters over QUIC + mutual TLS. Broadest
  span of the four — and the least mature.
- **hydra** is a clean, fast native-Rust OTP toolkit. If you want OTP semantics in a
  Rust binary and have **no** need for WASM isolation or multi-tenant sandboxing, it
  is an excellent, focused choice.
- **Lunatic** pioneered "Wasm instance = process" with supervisors and a distributed
  registry. It runs **core modules (preview1)** rather than the component model, has a
  Rust-centric SDK, and has seen **little activity since 2023 (v0.13)** — strong ideas,
  uncertain momentum.
- **wasmCloud** is a mature, **CNCF** project with genuine strengths: capability-based
  security, a NATS-based lattice for distribution, declarative deployment (`wadm`),
  good tooling (`wash`), and deep investment in the component-model/WASI standards. It
  targets distributed, horizontally-scaled component applications.

## Scored matrix (a structured opinion, not a measurement)

Scores are 1–5; weights in parentheses. They encode design judgement plus, where
noted, hands-on experience — **treat them as a discussion aid, not data.**

```
 Dimension (weight)                          │ rusm │ hydra │ lunatic │ wasmCloud
─────────────────────────────────────────────┼──────┼───────┼─────────┼──────────
 Actor / process model              (×2)      │ 5    │ 5     │ 5       │ 2
 Message-passing efficiency         (×1)      │ 5    │ 4     │ 4       │ 3
 Supervision / fault tolerance      (×2)      │ 5    │ 5     │ 4       │ 3
 WASM isolation / sandboxing        (×2)      │ 5    │ 1     │ 4       │ 5
 Capability security                (×1)      │ 4    │ 1     │ 3       │ 5
 Distribution / clustering          (×2)      │ 4    │ 4     │ 4       │ 5
 HTTP / WS / SSE serving            (×1)      │ 5    │ 3     │ 1       │ 3
 Guest languages / SDK ergonomics   (×1)      │ 5    │ 2     │ 3       │ 4
 Raw performance (spawn / msg)      (×1)      │ 5    │ 5     │ 4       │ 3
 Standards (component model / WIT)  (×1)      │ 5    │ 1     │ 2       │ 5
 Lifecycle control                  (×2)      │ 5    │ 5     │ 4       │ 2
 Maturity — on paper (CNCF/releases/docs)(×1) │ 2    │ 3     │ 2       │ 5
 Reliability in practice            (×2)      │ —    │ 4     │ 3       │ 3
 Operational model / deployment     (×1)      │ 3    │ 2     │ 3       │ 5
```

A deliberate choice: **"Maturity — on paper" and "Reliability in practice" are
separate rows.** Project signals (stars, releases, docs, a provider catalogue, CNCF
status) are leading indicators, *not* delivered reliability under a given workload —
the two can diverge. RUSM's "reliability in practice" is left **un-scored (—)** on
purpose: it has not been run hard enough, in enough places, to claim a number. That
blank is the honest statement of RUSM's central gap.

(We omit a single weighted total here: with one row deliberately blank and the
weighting admittedly RUSM-favouring, a headline number would imply more precision
than this exercise has.)

## Reading the matrix

- **RUSM** leads on the actor-model, supervision, component-isolation, serving and
  standards axes — it is the only one of the four that does OTP-style processes *and*
  component-model WASM *and* multi-language guests *and* first-class HTTP/WS/SSE in one
  runtime. Its weak axes are the ones that only time buys: maturity and proven
  reliability.
- **hydra** scores like RUSM on the pure-OTP axes and zero on WASM/sandboxing — which
  is correct: it isn't trying to do WASM. For native Rust OTP it is strong and proven
  for its scope.
- **Lunatic** shares RUSM's WASM-actor model but trails on the axes RUSM has since
  extended (component model, serving, TS guests) and is held back by low recent
  activity.
- **wasmCloud** leads on capability security, distribution, standards, ecosystem and
  operations — genuinely, and by a clear margin. Its lower scores are on the
  process/lifecycle and serving axes, for the architectural reasons below.

## On wasmCloud specifically

wasmCloud deserves credit first: it is a serious, well-engineered platform with the
**best-in-class** story here for capability security, multi-node distribution, the
component-model standards, and declarative operations. If your workload is a fleet of
**stateless, request-scoped components fanned out across a lattice**, it is arguably
the strongest option of the four.

The trade-off is architectural, and worth stating plainly because it is *checkable*
rather than anecdotal: wasmCloud's execution model is **invocation/request-scoped**,
with I/O **mediated by capability providers over the lattice**. That orientation is
excellent for stateless fan-out and weaker, by design, for **long-lived, stateful,
in-process connections** — which is the opposite of what a held WebSocket or a long SSE
stream wants. A platform that does not own a long-lived process lifecycle will tend to
bound invocation time and lean on providers for anything persistent.

A field note, **attributed and version-qualified, not a verdict**: in one team's
production use (a specific version and configuration), that orientation showed up as a
fixed per-invocation timeout (on the order of ~30s), pressure against an
instance-count ceiling under modest load, and meaningful friction implementing
WebSockets/SSE — leading them to avoid it for high-traffic, long-lived, streaming
workloads. This is one experience on one release; wasmCloud is actively developed and
its component-model and `wasi:http` support continue to advance, so treat it as a data
point about a design orientation, not a standing claim about the current project.

The takeaway is not "wasmCloud is bad" — it clearly isn't. It is that **paper maturity
is a leading indicator, not a guarantee for a particular workload**, and that
request-scoped/lattice platforms and lifecycle-owning process runtimes are suited to
different problems.

## How RUSM's design relates to those trade-offs

RUSM is deliberately shaped for the long-lived/stateful/streaming case — which is the
honest framing of "why it exists", not a claim that it is more battle-tested:

- It **owns process lifecycles** (the Wasm-free OTP core) and imposes **no
  execution-time cap** — a held WebSocket or a long SSE stream lives as long as it
  should, under supervision.
- Serving is **process-per-unit-of-work** (a fresh sandboxed instance per HTTP/SSE
  request, one process per WS connection), so head-of-line blocking is impossible and a
  crash drops one unit of work, never the server. Shared state lives in a long-lived
  `[components.<name>]` service (`resident = true`, reached over the actor API) or durable `kv`, never in the
  ephemeral serving instance.
- Its spawn path is **pooling + an on-demand overflow tier bounded by memory** rather
  than a fixed instance count, and instances are reclaimed on exit — so thousands of
  concurrent WS/SSE streams are bounded by RAM, not a fixed pool.
- **WS and SSE are first-class** (one sandboxed process per WS connection; SSE as a
  native streaming body), from RS *and* TS.

These are design answers. Whether they hold up under sustained production traffic is
exactly what RUSM has **not yet proven** — see below.

## Choosing between them

- **Native Rust OTP, no WASM needed** → **hydra**.
- **Stateless components fanned out across many nodes, with declarative ops and rich
  capability policy** → **wasmCloud**.
- **Long-lived, stateful, supervised WASM processes with sub-µs in-process messaging
  and first-class streaming, in one runtime** → **RUSM** — with eyes open to its
  maturity.
- **A WASM actor runtime today with an established (if quiet) codebase** → **Lunatic**,
  weighing its activity level.

## RUSM's honest caveats

- **Unproven at scale.** RUSM is young and largely single-author; it has not accrued
  the production mileage that turns sound design into earned trust. Every comparative
  strength above should be read next to this sentence.
- **Most published numbers are in-process / loopback.** They show the runtime is not
  the bottleneck; they are not network-throughput figures.
- **The matrix is judgement, not data.** It is a tool for reasoning about trade-offs,
  weighted toward RUSM's own priorities; re-weight it for yours.
