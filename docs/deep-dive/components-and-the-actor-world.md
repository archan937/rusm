# Concept — components & the `rusm:runtime` actor world

A RUSM process body can be a **WASI component** (the modern component-model
artifact — what `cargo component`, `jco`, and wasmCloud emit) or a **wasip1 core
module** (the older flat artifact Lunatic hosts). Both run instance-per-process as
real `rusm-otp` processes; they differ only in how they reach the runtime.

## The WIT actor world

A component imports the **`rusm:runtime/actor`** interface — a real WIT world bound
with `wasmtime::component::bindgen!`. That gives a guest, in *any* language, the
Erlang `Process` API as typed functions: `own-pid`, `send`, `receive` (async),
`list-processes`, `info`, `is-alive`, `kill`, `register`/`whereis`/`unregister`,
`set-label`. Each is a thin lift→call-`rusm-otp`→lower; the runtime stays the
source of truth, never reimplemented in the guest.

A core module gets the *same* operations as flat `rusm::*` imports that marshal
through linear memory (pointer + length) — see the [host ABI](/deep-dive/host-abi).

## Composition is message passing, not WIT wiring

This is the key design choice. Components do **not** link to each other through
WIT imports, and there is no lattice. They compose the Erlang way: spawn
instances, then `register`/`whereis` and `send`/`receive`. A request/reply
"callback" between two components is just a message and a reply — no new runtime
API, no static dependency graph.

## Why it matters

You get the component ecosystem (capabilities, language portability, WASI p1/p2/p3)
*on* the BEAM's process model — long-lived, addressable, supervised, preemptible,
killable, with **no execution-time cap**. The **component-storm** benchmark hosts
~440k component instances/sec.

> Shipped in Phase 7.
