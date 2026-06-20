# `bridges/` — one capability, one directory

A **bridge** is a single host capability owned end-to-end in one place. RUSM's own
platform capabilities live here *and* are authored in the exact same shape a custom
application bridge is — the platform is its own first consumer (dogfood), and adding or
changing a capability is one directory, not a hunt across four crates.

```
bridges/<name>/
  bridge.wit      # the contract (WIT interface slice)          → assembled into every world.wit
  host.rs         # native host impl + linker wiring             → compiled into rusm-wasm
  guest.rs        # Rust guest binding                           → synced into rusm-rs
  guest.go        # Go guest binding                             → synced into rusm-go
  guest.js        # TS/JS guest binding (QuickJS runtime bridge) → synced into the js-runner
```

## Performance is the decider — every bridge is typed, never marshaled

There is **no generic `bridge-call`/JSON dispatcher**. Every bridge — built-in or custom,
in any guest language — compiles to a **typed WIT host call**. The cost of supporting
custom bridges lives at **build time** (codegen + a js-runner rebuild when a TS guest uses
a custom bridge), never at **runtime**. Reorganizing into this layout emits identical
artifacts; the hot, fiber-coupled primitives keep their exact lowering to the instruction.

---

## Taxonomy

Bridges classify on **two orthogonal axes**: *what the guest sees* (3 functional types)
and *who authors it* (platform vs application).

### Axis 1 — functional type (what the guest sees)

| # | Type | Guest surface | `guest.*` | Inversion |
|---|---|---|---|---|
| 1 | **Polyfill** | a **standard** API the language/web/WASI already defines | a shim — or *nothing* (the stdlib itself) | guest calls it |
| 2 | **Introduced shape** | a **new rusm** API (Erlang/OTP, mostly) | the idiomatic per-language wrapper | guest calls it |
| 3 | **Transport / serving** | **none** — the host owns a protocol loop and *drives the guest* | none; it *feeds* types 1 & 2 | **it calls the guest** |

Type 3 is the one that has no `guest.*`: it doesn't expose an API, it turns a connection
into a `process` / `serve` handler. `wasip*` collapses into type 1 (from the guest's view
it *is* the stdlib; the host impl delegates to `wasmtime-wasi`). `serve` straddles 1↔2 —
the HTTP `fetch` handler is a polyfill, the WS/SSE `open/message/close` handlers are an
introduced shape. **Observability** (metrics/observer/attach) is *not* a bridge — it reads
runtime state for operators; it has no guest wiring.

### Axis 2 — authorship (platform vs application) → the 3 × 2 matrix

|                | **Platform** (rusm ships it) | **Application** (app authors it in *its* `bridges/`) |
|----------------|------------------------------|------------------------------------------------------|
| **Polyfill**   | `log`, `fetch`, `crypto`, `wasip*` | a `tracing`/`metrics` polyfill over the app's collector |
| **Introduced** | `process`, `pg`, `stream`, `kv`, `supervise` | `weather`, `db`, a native codec — RUSM's wasmCloud-provider answer |
| **Transport**  | `http`, `ws`, `sse`, `cluster` | rare — an app seldom owns a protocol loop |

The authorship axis *is* the platform-vs-application split, now structural: same directory
shape, different owner. A custom app bridge is mechanically identical to a platform one.

---

## Bridge inventory

`guest.*` = ✅ has guest bindings · ➖ host-only (no guest surface). **Status**: ✅ migrated to
`bridges/` · ⬜ still in the monolithic `actor` interface (migration in progress).

| Bridge | Type | Guest API | Host backing | Capability gate | `guest.*` | Bench gate | Status |
|---|---|---|---|---|:--:|---|:--:|
| `kv` | introduced | `kv.bucket(..)` | `rusm-kv` (redb) | `storage` | ✅ | `kv-storm` (ACID ceiling) | ✅ |
| `process` | introduced | `Pid`/`send`/`receive`/`spawn` | `rusm-otp` | `spawn`/`process-control` (per-op) | ✅ | `ping-pong`, `component-storm` | ⬜ |
| `pg` | introduced | `register_tag`/`whereis_tag`/`kill_tag` | `rusm-otp` | `process-control` (kill-tag) | ✅ | `pubsub-fanout` | ⬜ |
| `stream` | introduced | cross-process byte streams | `rusm-otp` | — | ✅ | `stream-pipe` | ⬜ |
| `supervise` | introduced | `monitor`/`supervise` | `rusm-otp` | `spawn` | ✅ | `fault-recovery` | ⬜ |
| `log` | polyfill | `console.*` / `log` / `slog` | `rusm-logfmt` | — (level-gated) | ✅ | — (not hot) | ⬜ |
| `serve` | polyfill + introduced | `fetch` handler / WS+SSE handlers | bridges below | — | ✅ | serving (below) | ⬜ |
| `http` | transport | — (drives handlers) | hyper | — | ➖ | `http-throughput` / loadtest | ⬜ |
| `ws` | transport | — (process per conn) | tokio-tungstenite | — | ➖ | `ws-echo` | ⬜ |
| `sse` | transport | — (stream per conn) | `wasi:http` body | — | ➖ | `sse-fanout` | ⬜ |
| `wasip1/2/3` | polyfill | the language stdlib | `wasmtime-wasi` | per-op (net/fs/…) | ➖ | — | ⬜ |
| `fetch` | polyfill | web `fetch()` | `wasi:http` / hyper | `network` | ✅ | — | ⬜ |
| `crypto` | polyfill | web `crypto.subtle` | RustCrypto | — | ✅ | `crypto-ops` | ⬜ |

---

## Single source of truth — `make sync-bridges`

`host.rs`/`guest.*` must physically live inside their (published) crates, so canonical
`bridges/` is **materialized** into them:

| Canonical | → generated copy (committed, drift-guarded) |
|---|---|
| `bridges/*/bridge.wit` | assembled into all rusm:runtime `world.wit` copies (`assemble-wit.sh`) |
| `bridges/*/host.rs`    | `crates/rusm-wasm/src/bridges/<name>.rs` |
| `bridges/*/guest.rs`   | `crates/rusm-rs/src/<name>.rs` |
| `bridges/*/guest.go`   | `packages/rusm-go/<name>.go` (+ regenerated wit-bindgen-go bindings) |
| `bridges/*/guest.js`   | `crates/rusm-wasm/js-runner/bridge/<name>.js` (the js-http-runner `include_str!`s it) |

**Edit the canonical file, then `make sync-bridges`.** Rust canonicals are
rustfmt-normalized first, so the synced copies survive a later `cargo fmt` byte-for-byte.
A drift test (`cargo test bridge_sync`, plus `go build`) fails the build if any copy
diverges; `make publish` re-syncs and aborts on any diff, so a stale binding can never
ship. Same pattern as `wit_in_sync` and `rusm-cli`'s `template::` tests.

---

## Adding / migrating a bridge — the checklist

1. `bridges/<name>/bridge.wit` — the interface slice (use `%name` to escape a reserved WIT
   keyword; a bridge that references shared types `use`s the `types` interface).
2. `bridges/<name>/host.rs` — `impl <name>::Host for WasiHost`; wire `add_to_linker`.
3. `bridges/<name>/guest.{rs,go,js}` — **all three** (or none, for a transport bridge).
   Polyfill → a standard-API shim; introduced → the idiomatic wrapper.
4. Register the new `world.wit` in `assemble-wit.sh`'s `TARGETS` map (the cross-check
   hard-fails on an unmapped one) and update consumer worlds (`component.wit`,
   js-http-runner `bindings`).
5. `make sync-bridges`, then `cargo test bridge_sync` + the full suite.
6. **Bench gate (mandatory): the bridge's scenario must stay flat** — see the inventory's
   *Bench gate* column. A regression means the migration is wrong; revert, don't ship.
