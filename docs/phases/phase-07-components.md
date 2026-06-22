# Phase 7 — component hosting

**Goal:** the payoff the whole design was built toward — **run real WASM
*components* as RUSM processes**: the component-model artifact and capabilities of
wasmCloud, but on RUSM's Erlang/OTP actor model, with no lattice and no
execution-time cap. **Graduates:** the **component-storm** scenario to live data.

## Why this matters

Phases 1–6 made the actor model real on native bodies and core-module Wasm. Phase
7 hosts the *modern* artifact: a WASI **component** (what `cargo component`, `jco`
and wasmCloud emit). A component becomes a long-lived, addressable, supervised,
killable, preemptible process — the BEAM model, for the component ecosystem.

## What we built (TDD throughout)

1. **`bridges/` over a shared core** — `rusm-wasm` adds `wasmtime-wasi` and a
   per-version bridge layout over one shared engine (epoch ticker, pooling
   allocator, CoW): `wasip1` (core modules), `wasip2` (components, the `@0.2.0`
   WASI interfaces), and `wasip3` (additive over p2 — the `@0.3.0` async/streams
   interfaces on the same component linker, with the async component model enabled
   in `Config`). A component importing p2 or p3 resolves against the one shared
   `WasiHost`.
2. **The `rusm:runtime` WIT actor world** (`wit/world.wit` + `bindgen!`) — a
   component imports `actor` and gets typed `self`/`send`/`receive`(async)/
   `list-processes`/`info`/`is-alive`/`kill`/`register`/`whereis`/`unregister`/
   `set-label`. Each host function is a thin call into `rusm-otp` — the Erlang
   `Process` API, callable from any language (Rust via `wit-bindgen`, TS via the
   genius-style Bun+rquickjs embed). Composition is **message passing**, not WIT
   wiring — no lattice.
3. **Default-deny capabilities** (`caps.rs`) — named profiles (`Sandboxed` /
   `NetworkClient` / `Trusted`) build a `WasiCtx` (fs preopens, env, network) plus
   a `StoreLimiter` memory cap. A process gets nothing unless granted.
4. **Introspection & byte streams** (`rusm-otp`, Wasm-free) — `list`/`info`/
   `set_label`, opt-in `mailbox_depth`, and `Received::Stream` over a
   Tokio-backpressured `StreamHandle`.
5. **App model** (`rusm-cli`) — `rusm.toml [components.<name>]`, a `./wasm/` loader that
   spawns each component under its profile, and `rusm build` / `rusm run` /
   `rusm dev` (one toolchain: `cargo build --target wasm32-wasip2`, no jco). Env is
   the Rust way: process env, then `.env`.
6. **Lifetime superiority** — a component runs as long as it needs, stays killable
   and preemptible (epoch), supervised — **no wasmCloud-style execution timeout**.
7. **The wasip1 bridge** (`bridges/wasip1.rs`) — RUSM on **Lunatic's home turf**:
   preview1 **core modules** run as processes too, with preview1 WASI, the same
   default-deny capabilities + `StoreLimiter`, the precomputed export index, and a
   **raw `rusm::*` actor ABI** marshalled through the guest's linear memory
   (`own_pid`/`send`/`receive`(async)/`list_processes`/`is_alive`/`kill`/`register`/
   `whereis`/`unregister`/`set_label`) — the *same* calls into `rusm-otp` as the WIT
   world, just a flat `(ptr, len)` calling convention. A misbehaving guest (bad
   pointer, no `memory`, non-UTF8 name) becomes a clean process crash, never a host
   panic.
8. **Cross-process byte streams** (`bridges/wasip1.rs`) — RUSM's stream-passing
   reaches guests: `stream_open(to)` hands a Tokio-backpressured `StreamHandle`'s
   read end to another process (it rides the mailbox as `Received::Stream`, the
   *same* primitive a native process gets) while the opener keeps the write end;
   `stream_write`/`stream_close` and `stream_accept`/`stream_read` move chunks with
   real back-pressure (a slow reader parks the writer's fiber, no busy-poll). This
   is composition the RUSM way — message-passing, not WIT inter-component wiring.

## Performance

The spawn path is deliberately optimized: pooling allocator + copy-on-write +
per-module `InstancePre` + a **precomputed export index** (no per-spawn by-name
lookup) + **opt-in mailbox depth** (default off → zero hot-path atomics) + a single
runtime-handle clone + park-based backpressure. The live **component-storm**
scenario sustains **~440k component spawns/sec** (p50 ~1 µs); the **module-storm**
scenario spawns the *same artifact Lunatic hosts* — wasip1 core modules — at
**~475k spawns/sec**. The two being so close is the point: **the component model
costs almost nothing over a raw core module** on RUSM's pooled path. The only big
step is to a bare task (~2.4M/sec) — that ~5x is the price of real Wasm memory
isolation, paid once whether you host a core module or a component. Lunatic hosts
only core modules with its own ABI — it has no component-model host at all.

## Concepts introduced

- [Components & the actor world](/deep-dive/components-and-the-actor-world) —
  the component model + the `rusm:runtime` WIT world; composition is message passing.
- [Permissions & sandboxing](/deep-dive/permissions-and-sandboxing) — per-process
  WASI capabilities, default-deny profiles.
- [Byte streams](/deep-dive/byte-streams) — cross-process, Tokio-backpressured.
- [The app model](/deep-dive/app-model) — `rusm.toml`, `./wasm/`, `rusm dev`.
- The full ABI — see the [host ABI](/deep-dive/host-abi).

## Play with it

```sh
cargo run --release -p rusm-bench -- run component-storm 3   # ~440k component spawns/sec
cargo run --release -p rusm-bench -- run module-storm 3      # ~475k wasip1 core-module spawns/sec
# In an app project (rusm.toml + components/ + wasm/):
rusm dev                                                     # build, then run the components
```

## Verification

`cargo test` green (component runs/reaped, trap → Crashed, epoch preempts a
component, memory-cap deny → Crashed, the full actor ABI driven by a real
`wit-bindgen` guest, component-to-component request/reply, manifest + loader);
component-storm live in the dashboard; workspace coverage ≥98%; the Wasm-free
invariant holds (no `wasmtime` under `rusm-otp`).

Cross-process byte streaming is live on **both** paths — wasip1 core modules (the
raw `rusm::*` ABI) and **components** (the `rusm:runtime` WIT world:
`stream-open`/`write`/`close`/`accept`/`read`, handle-based) — over the Wasm-free
`StreamHandle`.

**Deferred follow-ons:** a native p3-typed `stream<u8>` WIT signature (instead of
the handle-based ops) — *still* deferred as a standards-surface refinement, since the
handle ABI is load-bearing; and `rusm dev` filesystem watch/reload — **delivered in
[Phase 8](./phase-08-guest-ergonomics.md)**.

## Next

[Phase 8](./phase-08-guest-ergonomics.md): the **`rusm-rs` guest crate** — ergonomic
spawn/Mailbox/AbstractProcess/Supervisor over the raw ABI, so guests write idiomatic
code instead of hand-rolled bindings.
