# Concept — a process is a Wasm instance

In RUSM a *process* is a single WebAssembly instance running as a Tokio task. The
instance owns its linear memory (heap), its stack, and its set of permitted host
functions (syscalls). Nothing is shared with other processes.

## Why this is the right unit

- **Isolation = fault tolerance.** A trap (panic, out-of-bounds, `unreachable`)
  tears down only that instance. The host catches it and turns it into a process
  exit; linked processes and supervisors react (see
  [links & supervision](/deep-dive/links-and-supervision)).
- **Isolation = security.** Each instance only has the host functions and
  resources it was granted (see [permissions & sandboxing](/deep-dive/permissions-and-sandboxing)).
- **Cheap.** A fresh instance is small and fast to create (pooling allocator +
  copy-on-write + `InstancePre`) — RUSM sustains ~440k component spawns/sec, and
  ~2.4M for native bodies. Memory is bounded per process via Wasmtime store limits.

## How it maps to Tokio

`spawn(module)` instantiates the module and drives its entry function inside a
Tokio task. Because host calls are async (see
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)), a process that
"blocks" simply parks its task, freeing the worker thread for other processes.

> The process *abstraction* (task + mailbox, with an abort-based lifecycle and
> links/monitors) was built on native Rust bodies in Phases 1–3; since Phase 6 a
> process is a real isolated **Wasm instance** (core module or component) — the
> actor layer above is unchanged.
