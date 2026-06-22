# The process model

**In RUSM, a process is a single WebAssembly instance running as a Tokio task** — with
its own linear memory (heap), its own stack, and its own set of permitted host functions
(syscalls). Nothing is shared with any other process. This one decision is the foundation
everything else rests on, so it's worth seeing why an instance is the right unit.

## Why an instance is the right unit

**Isolation gives you fault tolerance.** A trap — a panic, an out-of-bounds access,
`unreachable`, a blown resource limit — unwinds only that one instance. The host catches it
and turns it into a process *exit*; linked processes and supervisors then react (see
[links & supervision](/deep-dive/links-and-supervision)). One process crashing can never
corrupt another.

**Isolation gives you security.** An instance can call only the host functions, and touch
only the resources, it was explicitly granted (see
[permissions & sandboxing](/deep-dive/permissions-and-sandboxing)) — default-deny, per
process.

**And it's cheap.** A fresh instance is small and fast to create — a pooling allocator,
copy-on-write memory, and a precomputed `InstancePre` keep spawning on the hot path. RUSM
sustains ~440k component spawns/sec (and ~2.4M/sec for native process bodies), with memory
bounded per process by Wasmtime store limits. Isolation this cheap is isolation you can
afford to use everywhere — a process per request, per connection, per unit of work.

## How a process maps to Tokio

`spawn(module)` instantiates the module and drives its entry function inside a Tokio task.
Because host calls are async under the hood (see
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)), a process that "blocks"
— waiting on a message, say — simply parks its task and frees the worker thread for another
process. The result is hundreds of thousands of concurrent processes multiplexed over a
handful of OS threads.

> The process *abstraction* — task + mailbox, an abort-based lifecycle, links and monitors —
> was first built on native Rust bodies in Phases 1–3. Since Phase 6 a process is a real
> isolated **Wasm instance** (core module or component); the actor layer above it never
> changed.
