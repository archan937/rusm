# Components & the actor world

RUSM hosts WebAssembly the modern way — as **components** — but composes them the Erlang
way. A process body can be a **WASI component** (the component-model artifact
`cargo component`, `jco`, and wasmCloud emit) or an older **wasip1 core module** (the flat
artifact Lunatic hosts). Either way it runs instance-per-process as a real `rusm-otp`
process; the two differ only in *how* they reach the runtime.

## The `rusm:runtime` actor world

A component imports the **`rusm:runtime/actor`** interface — a real WIT world bound
with `wasmtime::component::bindgen!`. That hands a guest, in *any* language, the
Erlang `Process` API as typed functions: `own-pid`, `send`, `receive` (async),
`list-processes`, `info`, `is-alive`, `kill`, `register`/`whereis`/`unregister`,
`set-label`. Each is a thin lift → call-`rusm-otp` → lower, so the runtime stays the
single source of truth — never reimplemented in the guest.

A core module gets the *same* operations as flat `rusm::*` imports that marshal
through linear memory (pointer + length) — see the [host ABI](/deep-dive/host-abi).

## Composition is message passing, not WIT wiring

This is the design choice that sets RUSM apart. Components do **not** link to each other
through WIT imports, and there is no lattice. They compose the Erlang way: spawn
instances, then `register`/`whereis` to find each other and `send`/`receive` to talk. A
request/reply "callback" between two components is just a message and a reply — no new
runtime API, no static dependency graph to maintain.

## Why it matters

You get the whole component ecosystem — capabilities, language portability, WASI
p1/p2/p3 — running *on* the BEAM's process model: long-lived, addressable, supervised,
preemptible, killable, with **no execution-time cap**. The **component-storm** benchmark
hosts ~440k component instances/sec.

> Shipped in Phase 7.
