# Component lifecycles — overview

Every component in RUSM — an HTTP handler, a WebSocket worker, an SSE stream, a
per-call worker, a resident service — is the **same runtime object: a process**. One
Wasm instance = one Tokio task, one mailbox, one fiber, one capability set. The
lifecycle is therefore *uniform*; what differs by type is only **when** a process is
spawned and **how** it ends.

This overview covers what's shared — the two domains, the phases, and the failure
vocabulary. Then there's **a chapter per component type**, each walking that type's
events (normal, disconnect, connection error, crash, memory/OOM crash, kill):

- [**HTTP component**](/deep-dive/lifecycle-http) — a fresh instance per request.
- [**SSE component**](/deep-dive/lifecycle-sse) — a per-request streaming feed.
- [**WebSocket component**](/deep-dive/lifecycle-websocket) — one process per connection.
- [**Worker component**](/deep-dive/lifecycle-worker) — non-serving, spawned per call.
- [**Service component**](/deep-dive/lifecycle-service) — non-serving, resident and stateful.

(A stock [`wasi:cli` command](#other-wasi-cli-command-components) component is covered
briefly at the end.)

## The two domains

The lens that matters most is the **two domains** the lifecycle runs through:

- **Platform domain** — what **RUSM owns**: routing, spawning, the scheduler and
  fibers, the mailbox, socket ownership, the reply/stream plumbing, capability and
  memory enforcement, exit detection, links/monitors, and the supervisor. You never
  write this.
- **Application domain** — what **you write**: the handler action, the service
  functions, the worker body. Pure logic — no router, no wire, no socket, no `async`,
  no lifecycle bookkeeping.

The boundary is deliberate and load-bearing: a failure in the application domain is
*caught by the platform domain* and turned into a status code, a `Down` message, or a
supervisor restart — it is never allowed to take down the listener, a sibling, or the
node. Every per-type chapter is organised around this split: **"Platform owns / You
write,"** then an events table whose columns are *Platform domain* and *Application
domain*.

## Who owns what, phase by phase

| Phase | Platform domain (RUSM) | Application domain (you) |
| --- | --- | --- |
| **Register** | resolve a name → a prepared component (compiled module + linked imports) | declare `[[serve]]` / `[components.<name>]` / `[serve.routes]` |
| **Spawn** | instantiate on the pooling allocator + copy-on-write + `InstancePre` (~440k/s); apply default-deny **capabilities** + a `StoreLimiter` memory ceiling; create the mailbox, fiber, and abort handle | — |
| **Dispatch** | match the route, own the socket, spawn the handler, send the request over the actor wire, run the ephemeral reply *responder* | — |
| **Run** | suspend/resume the fiber on every blocking call (`receive`, `Stream::read`, a service `call`); enforce [epoch preemption](/deep-dive/epoch-preemption) on CPU-bound guests | your action / service / worker body — straight-line, blocking-looking code |
| **Reply / stream** | turn your `Response` into the HTTP response; drain your byte stream into the chunked body with back-pressure; own the WS socket sink | `return` a `Response`, or `sse.data(…)`, or `conn.send(…)` |
| **Exit** | classify the exit (Normal / Crashed / Killed), reclaim the instance + mailbox + streams, fire links and monitors, drive the supervisor | optionally `monitor` / `Supervisor` / `trap_exit` |

**Blocking is async, for free.** In the Run phase your code can call `receive` or read
a stream and *block* — the platform suspends the [fiber](/deep-dive/fibers-and-blocking-to-async)
and frees the scheduler thread until data arrives, then resumes you. You write
sequential code; the platform makes it cooperative.

## The shared failure vocabulary

Every component ends in one of these ways. The per-type chapters refer back to these
definitions rather than repeat them.

| Outcome | What it is | Domain that originates it |
| --- | --- | --- |
| **Normal exit** | your entrypoint returned | application (clean finish) |
| **Crash (trap)** | a guest fault — `panic!`, `.unwrap()` on `None`/`Err`, `unreachable`, an out-of-bounds access, or a capability violation that traps. Wasmtime raises a trap; the platform marks the process **Crashed** with the trap as the exit reason | application fault, **caught** by the platform |
| **Memory crash (OOM)** | the guest tries to grow linear memory past its `StoreLimiter` ceiling (the capability profile's `max-memory-mb`). The allocation fails → trap → **Crashed**. It is an ordinary crash — bounded to that one instance, never the node | application fault, **bounded** by the platform |
| **Kill** | an external `kill`, or an exit cascade from a crashed **link**. The platform fires the process's `futures` abort handle; the signal rides the same mailbox (no second channel) | platform / supervision |
| **Client disconnect** | the peer closed the socket. **Not a crash** — the platform surfaces it (a failed write or a closed request body) and the unit ends cleanly | platform (detected), application (observes via a `false` write) |
| **Connection error** | a socket- or protocol-level failure *around* your code: a malformed request line, an unreadable body, a write to a half-open socket. Usually your code never runs — the platform answers directly | platform |

On any exit the platform **reclaims** the instance (returned to the pool), the mailbox,
and any byte streams, then notifies **links** (an exit cascade, unless the peer set
`trap_exit`) and **monitors** (a `Down` carrying the reason). A **supervisor** restarts
per its strategy (one-for-one / one-for-all / rest-for-one) under
[windowed restart-intensity](/deep-dive/links-and-supervision) — too many restarts in the
window and it escalates instead of crash-looping.

## Why a failure stays local

The two-domain boundary is what makes the blast radius of any failure exactly **one
unit of work** — the guarantee every chapter relies on:

- **Isolation by instance (platform).** Each request/connection/worker is its own Wasm
  instance with its own linear memory and capabilities. A trap or OOM can corrupt only
  that instance's memory, which is discarded on exit.
- **No head-of-line blocking (platform).** Serving is process-per-unit, so a slow,
  stuck, or crashed handler never holds up the listener or other clients.
- **Crash containment (platform).** An application-domain fault becomes a `Crashed`
  exit — surfaced as a **502**, a dropped connection, or a `Down`/exit signal — never a
  node-wide failure. Per-request work just gets a fresh instance; resident work is
  restarted (or escalated) by a supervisor.
- **No leaks, no spins (platform).** Exit reclaims the instance, mailbox, and streams;
  byte streams are bounded and back-pressured, so a producer parks rather than spins,
  and a disconnect always tears the process down.

You write the application domain and reason about one request at a time; the platform
domain guarantees the rest.

## Other: `wasi:cli` command components

A stock command component (`WasmRuntime::spawn_command`) has no actor world — just
`wasi:cli/run`. **Normal:** runs `run`, exits with code **0**. **Crash / non-zero exit
/ OOM:** **Crashed**, the exit reason carrying the status. Same isolation and reclaim
as any process; it just has no serving or messaging role.

See also: [the process model](/deep-dive/wasm-instance-as-process),
[links & supervision](/deep-dive/links-and-supervision),
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async),
[permissions & sandboxing](/deep-dive/permissions-and-sandboxing), and
[the serving model](/deep-dive/serving-model).
