# Phase 7 — Component hosting

The modern artifact: a WASM component, running as a supervised, addressable, killable, preemptible process — the Erlang/OTP model, applied to the WebAssembly component ecosystem.

## Why this phase

Phase 6 proved a Wasm instance can be a supervised process. Phase 7 hosts the *modern* artifact: a WASM component compiled with `cargo build --target wasm32-wasip2`, the same target wasmCloud and the broader component ecosystem use. But where wasmCloud executes components in a lattice with a 30-second execution cap, RUSM runs them as long-lived, addressable processes — supervised, killable, preemptible, no timeout.

The programming model stays identical to Phase 6. The component model adds structured types, WASI interfaces, and language-agnostic ABI — RUSM just makes those components the process body.

## What shipped

1. **`bridges/` over a shared core** — `rusm-wasm` adds `wasmtime-wasi` and per-version bridges over one shared engine (epoch ticker, pooling allocator, CoW): `wasip1` (core modules, Lunatic's home turf), `wasip2` (components, WASI `@0.2.0`), and `wasip3` (additive over p2 — WASI `@0.3.0` async interfaces on the same component linker).
2. **The `rusm:runtime` WIT actor world** — a component imports the `actor` interface and gets typed `self`/`send`/`receive`/`list-processes`/`info`/`kill`/`register`/`whereis`/`unregister`/`set-label`. Each is a thin call into `rusm-otp` — the Erlang `Process` API, callable from any language that targets WASI components.
3. **Default-deny capabilities** (`caps.rs`) — named profiles (`Sandboxed` / `NetworkClient` / `Trusted`) build a `WasiCtx` (fs preopens, env, network) plus a `StoreLimiter` memory cap. A process gets nothing unless granted.
4. **Cross-process byte streams** (`Received::Stream`, Wasm-free) — `stream-open(to)` hands a Tokio-backpressured `StreamHandle` to another process via the mailbox; the opener keeps the write end. Real back-pressure: a slow reader parks the writer's fiber — no busy-poll, no unbounded buffering.
5. **App model** (`rusm-cli`) — `rusm.toml [components.<name>]`, a `./wasm/` loader, and `rusm build` / `rusm run` / `rusm dev`. One toolchain: `cargo build --target wasm32-wasip2`, no jco, no cargo-component.
6. **wasip1 core-module bridge** — RUSM on Lunatic's home turf: preview1 core modules run as processes too, with the same default-deny caps + `StoreLimiter` and the same raw `rusm::*` actor ABI over linear memory.

## Design highlights

- **Composition is message passing, not WIT wiring.** Two components communicating don't share a WIT import — they send messages. This keeps the programming model identical whether processes are on one node or across a cluster (Phase 9). There is no lattice, no link map, no runtime topology to declare.
- **Component model costs almost nothing over a raw core module.** Component-storm: **~440k spawns/sec**. Module-storm (wasip1 core modules — the direct Lunatic comparison): **~475k spawns/sec**. The ~5× gap to bare tasks (~2.4M/sec) is the price of real Wasm memory isolation — paid once, regardless of whether you host a core module or a full component.
- **No execution cap.** A component runs as long as it needs. `rusm dev` keeps running; residents stay alive. The epoch preemption from Phase 6 ensures a spinning guest doesn't starve others, but it never *kills* a guest for being slow.
- **Per-process `InstancePre` with precomputed export index.** No per-spawn by-name export lookup. The entry function index is resolved at prepare time, so every spawn hits the same fast path.

## What this unlocks

Any language that compiles to `wasm32-wasip2` can now be a supervised RUSM process. Rust, TypeScript (via Phase 8's rquickjs runner), Go via TinyGo — all compile to the same artifact, run under the same actor model, message each other over the same wire.

The full OTP tree — spawn, supervise, kill, link, monitor, registry — applies to components without modification. Crash a component and the supervisor restarts it. Register it by name and any other component can find it. The app model (`rusm.toml`, `rusm build`, `rusm dev`) makes this declarative.

## Try it

```sh
cargo run --release -p rusm-bench -- run component-storm 3   # ~440k component spawns/sec
cargo run --release -p rusm-bench -- run module-storm 3      # ~475k wasip1 core-module spawns/sec
# In an app project:
rusm build && rusm dev                                       # build components, run, watch for changes
```

## Status

Phase complete. Component-storm and module-storm are live in the dashboard. The Wasm-free invariant holds. Deferred to Phase 11: a native p3-typed `stream<u8>` WIT signature (the handle-based byte streams are fully functional and load-bearing; the native signature is a standards-surface refinement).

---

*Next: [Phase 8](./phase-08-guest-ergonomics.md) — guest ergonomics: TypeScript, Rust, and Go SDKs, the typed client, streaming, callbacks, and `rusm dev` watch+reload.*
