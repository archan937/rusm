# Changelog

All notable changes to RUSM are documented here. The project is a Cargo workspace of
several crates plus the `rusm-ts` npm package; because they version independently, each
release lists the versions it shipped. Format follows
[Keep a Changelog](https://keepachangelog.com/); the project is pre-1.0, so minor/patch
numbers don't yet imply SemVer guarantees.

## [0.1.4] — 2026-06-16

### Added
- **Process groups (Erlang `pg`)** — the scoped-cancellation primitive. A process tags
  *itself* with `register_tag(tag)` / `unregister_tag(tag)`; `whereis_tag(tag)` lists live
  members; `kill_tag(tag)` terminates the whole group. One tag → many pids, one pid → many
  tags; memberships are reaped on exit by the same reaper that releases names, and tags add
  **zero hot-path cost**. `kill_tag` is gated by the `process-control` capability (like
  `kill`); self-tagging is unprivileged. Available from both guests — `register_tag` /
  `kill_tag` / `whereis_tag` in **rusm-rs**, `Process.registerTag` / `killTag` /
  `whereisTag` in **rusm-ts** — backed by the Wasm-free `rusm-otp` core.
- **`rusm new --lang generic`** — scaffold a bring-your-own-component app; `rusm build`
  copies a pre-built wasip2 `.wasm` into `wasm/` (prefers `<name>.wasm`, errors on
  ambiguity). (#3, thanks @thomas9911)

### Changed
- **pico-args** is now the single argument parser for the `rusm` CLI. (#2, thanks @thomas9911)
- Dropped the `xtask` crate; `make docs-deploy` now publishes the VitePress site to
  `gh-pages` directly — one fewer crate to build.

### Fixed
- The benchmark crate now builds on **Windows** (the unix-only `rlimit` soft-limit lookup
  is gated; Windows falls back to a 256-fd ceiling). (#1, thanks @thomas9911)

### Versions
rusm-otp 0.1.2 · rusm-wasm 0.1.4 · rusm-cli 0.1.4 · rusm-rs 0.1.2 · rusm-rs-macros 0.1.2 ·
rusm-ts 0.1.2. rusm-logfmt / rusm-kv unchanged at 0.1.0.

## [0.1.3] — 2026-06-15

A large release: durable storage, platform logging, declarative routing, and a warm-start
TS runner. First crates.io publish of **rusm-kv** and **rusm-logfmt**.

### Added
- **Durable key-value storage** for guests — the Wasm-free `rusm-kv` crate (embedded redb)
  behind the `storage` capability and a `kv-*` ABI.
- **`receive-timeout`** for guests (Erlang's `receive … after`).
- **pub/sub fan-out** as a primitive (`rusm-rs` `pubsub::Topics`) and **turnkey offloaded
  SSE** live fan-out, proven end-to-end.
- **Platform logging** — guests log via `console.*` (TS) / the `log` crate (Rust) to the
  host log, with a serving **access log**; gated by `[log] level`, no stdio wiring in guest
  code. Shared column widths + timestamps via `rusm-logfmt`.
- **Routed, per-request serving** — `#[rusm_rs::handlers]` named actions, a per-request
  HTTP/SSE gateway over the actor world, and SSE streaming handlers in the unified model.
- **`crypto.subtle`** for TS guests (RustCrypto), `btoa`/`atob`, capability-granted
  `process.env`, and dynamic bundle sourcing (load a component's JS from a URL or `kv`).
- `rusm serve` also brings up `[[components]]` (one node, one command).

### Changed
- **Breaking:** a registered component runs under its **own declared capability profile**
  (not the spawner's).
- **Breaking:** `[components.<name>]` is now a keyed map with a `resident` flag
  (boot-spawned + supervised), replacing the dead `restart` key.
- **Breaking:** routes are scoped to their listener — `[serve.routes]` (was a global
  `[routes]`).
- **Breaking:** disabled the component-model-async ABI to stop a `wasi:io` busy-poll.

### Performance
- **wizer pre-initialization** of the js-runner and js-http-runner — every TS guest
  CoW-starts *warm* (engine + bridge booted once at build time); ~8× TS HTTP throughput,
  still instance-per-request. Closed-loop benchmark load made steady at all speeds.

### Fixed
- Outbound `fetch` parks instead of busy-polling (kills an 800% CPU / server-stall).
- `js-runner` dispatched the request before writing the body (POST bodies were lost).
- `rusm-logfmt` inherits license/repository from the workspace (publish-blocking metadata).

### Versions
rusm-wasm 0.1.3 · rusm-cli 0.1.3 · rusm-otp 0.1.1 · rusm-node 0.1.1 · rusm-rs 0.1.1 ·
rusm-rs-macros 0.1.1 · rusm-ts 0.1.1 · **rusm-kv 0.1.0 (new)** · **rusm-logfmt 0.1.0 (new)**.

## [0.1.2] — 2026-06-10

### Fixed
- A string `Response` now defaults to `Content-Type: text/plain;charset=UTF-8`.

### Versions
rusm-cli 0.1.2 · rusm-wasm 0.1.2.

## [0.1.1] — 2026-06-10

### Fixed
- `rusm-wasm` UTF-8 hotfix.

### Versions
rusm-cli 0.1.1 · rusm-wasm hotfix.

## [0.1.0] — 2026-06-10

Initial crates.io release — the RUSM runtime, guest SDKs, and CLI, built through Phase 11.

### Added
- **`rusm-otp`** — the Wasm-free Erlang/OTP core: lightweight processes (one Tokio task
  each), message passing, links/monitors/exit signals, `trap_exit`, supervision with
  restart intensity, a sharded named registry, timers, graceful shutdown, TCP
  (process-per-connection), introspection, and back-pressured byte streams.
- **`rusm-wasm`** — the Wasmtime component host: instance-per-process guests over the
  component model (WASI p1/p2/p3), the `rusm:runtime` WIT actor world, default-deny
  capability profiles + a memory limiter, and an optimized spawn path (pooling allocator +
  CoW + per-module `InstancePre`).
- **`rusm-cluster`** — Wasm-free distributed transport over QUIC + mutual TLS (cross-node
  send, gossiped global registry, remote spawn, live attach).
- **Guest SDKs** — `rusm-rs` / `rusm-rs-macros` (Rust) and the embedded TS runner; plus
  `rusm-wire` and `rusm-node`.
- **`rusm-cli`** — the `rusm` binary: `new` / `build` / `run` / `dev` / `serve` /
  `node start` / `attach`.

### Versions
rusm-otp · rusm-wasm · rusm-node · rusm-cluster · rusm-rs · rusm-rs-macros · rusm-wire ·
rusm-cli. (rusm-kv and rusm-logfmt were first published in 0.1.3.)

[0.1.4]: https://github.com/archan937/rusm/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/archan937/rusm/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/archan937/rusm/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/archan937/rusm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/archan937/rusm/releases/tag/v0.1.0
