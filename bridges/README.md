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
  guest.go        # Go guest binding                            → synced into rusm-go
  guest.js        # TS/JS guest binding (QuickJS runtime bridge) → synced into the js-runner
  guest.d.ts      # published TS types (the .d.ts app authors consume) → synced into rusm-ts
```

**The shape is presence-based, not fixed — it varies by type** (`sync-bridges` copies each
file only `if` present). A transport bridge is `host.rs` only; a shared-types interface is
`bridge.wit` only; a TS-only polyfill is `host.rs` + `guest.js`/`guest.d.ts`. See the
inventory's *files* column.

## Performance is the decider — every bridge is typed, never marshaled

There is **no generic `bridge-call`/JSON dispatcher**. Every bridge — built-in or custom,
in any guest language — compiles to a **typed WIT host call**. The cost of supporting
custom bridges is **build-time** (codegen + a js-runner rebuild when a TS guest uses a
custom bridge), never **runtime**. Reorganizing into this layout emits identical artifacts;
the hot, fiber-coupled primitives keep their exact lowering to the instruction.

---

## Taxonomy — two orthogonal axes

### Axis 1 — functional type (what the guest sees)

| # | Type | Guest surface | Inversion |
|---|---|---|---|
| 1 | **Polyfill** | a **standard** API the language/web/WASI already defines | guest calls it |
| 2 | **Introduced shape** | a **new rusm** API (Erlang/OTP, mostly) | guest calls it |
| 3 | **Transport / serving** | **none** — the host owns a protocol loop and *drives the guest* | **it calls the guest** |

Type 3 has no `guest.*`: it turns a connection into an `actor` / `serve` handler. `wasip*`
collapses into type 1 (from the guest's view it *is* the stdlib; the host delegates to
`wasmtime-wasi`). `serve` straddles 1↔2 (HTTP `fetch` handler = polyfill; WS/SSE handlers =
introduced). **Not bridges:** a shared-**`types`** interface (a WIT type vocabulary — no
impl, no capability) is *supporting* infrastructure; **observability** (metrics/observer/
attach) reads runtime state for operators; **`cluster`** is the distribution layer (a whole
crate — QUIC/TLS/gossip — that transparently extends `actor` cross-node), not a bridge.

### Axis 2 — authorship → the 3 × 2 matrix

|                | **Platform** (rusm ships it) | **Application** (app authors it in *its* `bridges/`) |
|----------------|------------------------------|------------------------------------------------------|
| **Polyfill**   | `log`, `fetch`, `crypto`, `wasip*` | a `tracing`/`metrics` polyfill over the app's collector |
| **Introduced** | `actor`, `pg`, `streams`, `kv`, `serve` | `weather`, `db`, a native codec — RUSM's wasmCloud-provider answer |
| **Transport**  | `http`, `ws`, `sse` | rare — an app seldom owns a protocol loop |

The authorship axis *is* the platform-vs-application split, made structural: same directory
shape, different owner. A custom app bridge is mechanically identical to a platform one.

---

## Bridge inventory

**files**: which of `bridge.wit`(W) / `host.rs`(H) / `guest.{rs,go,js,d.ts}`(R/G/J/T) the dir
carries. **Status**: ✅ canonical in root `bridges/` (cross-crate) · 🏠 single-source in
`crates/rusm-wasm/src/bridges/` (host-only — see *Where a bridge's code lives*).

| Bridge | Type | Guest API | Host backing | Gate | Files | Bench gate | Status |
|---|---|---|---|---|---|---|:--:|
| `types` | *(supporting)* | — (shared `pid`) | — | — | W | — | ✅ |
| `actor` | introduced | `Pid`/`send`/`receive`/`spawn`/`monitor`/`supervise` | rusm-otp | `spawn`/`process-control` (per-op) | W H R G J | `ping-pong`, `component-storm`, `fault-recovery` | ✅ |
| `pg` | introduced | `register_tag`/`whereis_tag`/`kill_tag` | rusm-otp | `process-control` (kill-tag) | W H R G J | `pubsub-fanout` | ✅ |
| `streams` | introduced | cross-process byte streams | rusm-otp | — | W H R G J | `stream-pipe` | ✅ |
| `kv` | introduced | `kv.bucket(..)` | rusm-kv (redb) | `storage` | W H R G J T | `kv-storm` (ACID ceiling) | ✅ |
| `log` | polyfill | `console.*` / `log` / `slog` | rusm-logfmt | — (level) | W H R G J | — (not hot) | ✅ |
| `serve` | polyfill + introduced | `fetch` / WS+SSE handlers | http/ws/sse | — | W H R G J | serving | ✅ |
| `http` | transport | — (drives handlers) | hyper | — | H | `http-throughput` / loadtest | 🏠 |
| `ws` | transport | — (process per conn) | tokio-tungstenite | — | H | `ws-echo` | 🏠 |
| `sse` | transport | — (stream per conn) | `wasi:http` body | — | H | `sse-fanout` | 🏠 |
| `wasip1/2/3` | polyfill | the language stdlib | wasmtime-wasi | per-op | H | — | 🏠 |
| `fetch` | polyfill (TS-only) | web `fetch()` | `wasi:http` / hyper | `network` | H J | — | 🏠 |
| `crypto` | polyfill (TS-only) | web `crypto.subtle` | RustCrypto | — | H J | `crypto-ops` | 🏠 |

### Where a bridge's code lives

The root `bridges/` canonical-copy machinery exists to **eliminate cross-crate duplication**:
a capability bridge's `guest.{rs,go,js}` are materialized into three *other* published crates
(rusm-rs / rusm-go / rusm-ts), so a single canonical source per language is the only way to
stop them drifting. That's why ✅ bridges live in root `bridges/`.

**Host-only bridges (🏠) have no cross-crate spread** — their code lives entirely in
`crates/rusm-wasm` (transport loops) or the js-runner (the TS `fetch`/`crypto` polyfills). It
is *already* single-source there, so it stays in `crates/rusm-wasm/src/bridges/` — copying it
into root `bridges/` would **duplicate it for no benefit** (a DRY violation), so we don't. The
root dir is for cross-crate bridges; `src/bridges/` is rusm-wasm's host code (the 🏠 bridges +
the synced ✅ `host.rs` copies). Note `wasip2.rs`/`wasip1.rs` are the **component/core-module
runtime** (spawn path, linker assembly), not "the wasip bridge" — the WASI wiring is a few
`add_to_linker` lines within them; and `conn`/`compress`/`tls`/`ws_codec`/`access` are
**support modules** for the transport bridges, not bridges themselves.

> `actor` is the irreducible Erlang core (it keeps the interface name `actor`, so there is no
> collision with `world process`). `pg`/`streams`/`log`/`serve` split out of it as siblings;
> `kv` already has. Only **`pid`** is shared across interfaces → the minimal `types`
> interface; `process-info`/`connection-info`/`stream-id` are each used by one interface and
> stay local.

---

## Shared types — the `types` interface

WIT gives each interface its own type namespace, so a `pid` defined in `actor` is a
*different* type from one defined in `stream` — you couldn't pass a pid from `actor.own-pid`
to `stream.stream-open`. The fix is one **`types`** interface holding the genuinely-shared
vocabulary (today just `type pid = u64`); every interface that needs it writes
`use types.{pid};` at the top of its `bridge.wit`.

The assembler distinguishes it **automatically — no marker**: an interface that declares a
`func` is a capability and is `import`ed into the `process` world; an interface with **no
`func`** (types-only) is emitted into the package but **not imported** (it is reached via
`use`). So `bridges/types/` is `bridge.wit`-only and never appears in the world's import list.

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
| `bridges/*/guest.js`   | `crates/rusm-wasm/js-runner/bridge/<name>.js` (js-http-runner `include_str!`s it) |
| `bridges/*/guest.d.ts` | `packages/rusm-ts/` (the published type surface) — *planned; needs rusm-ts split into per-bridge type modules, see Guards* |

**Edit the canonical file, then `make sync-bridges`.** Rust canonicals are
rustfmt-normalized first, so the synced copies survive a later `cargo fmt` byte-for-byte. A
drift test (`cargo test bridge_sync`) fails the build if any copy diverges; `make publish`
re-syncs and aborts on any diff, so a stale binding can never ship.

### Guards

- **Rust / TS source**: byte-for-byte `include_str!` drift test (`cargo test bridge_sync`). ✅
- **WIT**: `assemble-wit.sh --check` (regenerate-and-diff every copy). ✅
- **Go bindings**: `make sync-bridges` regenerates them and `make publish` aborts on any
  resulting `git diff` — so a drifted binding can't ship (byte-exact at publish). ✅
- **js-runner `.wasm`**: the `guest.js` *source* is drift-guarded (above); the `.wasm` itself
  is a **prebuilt artifact** (QuickJS + bridge JS, wizer-snapshotted). wizer's output is **not
  byte-reproducible**, so it can't be drift-checked like source — rebuild it with
  `make build-runtimes` after editing a bridge `guest.js` (same prebuilt-artifact model as the
  test fixtures). ✅ *(mitigated; non-determinism precludes an exact check)*
- **rusm-ts published types**: the `.d.ts` surface is still hand-maintained in
  `packages/rusm-ts/index.ts`. Single-sourcing it from `bridges/<name>/guest.d.ts` is folded
  into the **custom-bridge TS codegen** (which generates `.d.ts` from `bridge.wit`). ⬜

---

## Migration plan — one breaking release

Each split is a breaking ABI change (guests rebuild `actor.kv-*` → `kv.*`, etc.). **All
remaining splits land in a single 0.4.0** — never serial breaking releases. kv's split is
committed but unreleased; `actor`/`pg`/`stream`/`log`/`serve` join it before 0.4.0 ships, so
guests rebuild exactly once.

## Adding / migrating a bridge — the checklist

1. `bridges/<name>/bridge.wit` — the interface slice (`%name` escapes a reserved WIT keyword;
   `use types.{pid};` for shared types).
2. `bridges/<name>/host.rs` — `impl <name>::Host for WasiHost` + `add_to_linker`. **Shared
   host helpers stay on `WasiHost`** (e.g. `kv_bucket`, `resolve_local`); only the
   interface's own logic lives in the bridge file (SoC).
3. `bridges/<name>/guest.{rs,go,js,d.ts}` — **all** (or none, for a transport bridge).
   Polyfill → a standard-API shim; introduced → the idiomatic per-language wrapper.
4. Register the new `world.wit` in `assemble-wit.sh`'s `TARGETS` map (the cross-check
   hard-fails on an unmapped one); update consumer worlds (`component.wit`, js-http-runner
   `bindings`).
5. `make sync-bridges`, then `cargo test bridge_sync` + the full suite + `go build`.
6. **Bench gate (mandatory, baseline-first):** capture the scenario's number *before* the
   split, then compare the **median of N runs** after (benches are noisy). The bridge's
   scenario must stay flat — see the inventory's *Bench gate* column. A regression means the
   migration is wrong; **revert, don't ship**.
