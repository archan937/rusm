# Development guide

This page is for anyone contributing to RUSM — or building on its core — and it
captures how the project is actually built day to day: the toolchains, the commands,
where the tests live, and the recipe for extending the host. The rigor here is the
point, not paperwork: TDD-always, a ≥98% coverage floor, and a strict Wasm-free-core
boundary are exactly what keep the codebase reference-quality and trustworthy as it
grows. Read this before your first change and the rest of the repo will feel familiar.

## Principles

- **TDD always.** Write the failing test first, then implement until green. Baby
  steps — one concept at a time.
- **Coverage: aim for 100%** (≥98% floor). Only genuinely-unreachable invariant
  guards are acceptable gaps, and they should be obvious from the code.
- **Comments only for critical info.** No comments restating obvious code.
- **Senior, idiomatic, DRY, well-separated.** Self-review every change.
- **Respect the Wasm-free-core boundary.** The `rusm-otp` core (processes,
  messaging, supervision, registry, scheduler) must never depend on Wasmtime — all
  Wasm lives in `rusm-wasm`. The dependency graph enforces it; see
  [architecture](/about/architecture) for why this invariant matters.

## Platform

Developed and tested on **macOS and Linux**. The workspace also **builds on
Windows** — the connection benchmarks fall back to a 256-fd ceiling there, since
the `rlimit::Resource` soft-limit lookup is unix-only. Windows is not yet a
verified runtime target.

**Guest build toolchains.** Rust guests need the `wasm32-wasip2` target (`rustup target
add wasm32-wasip2`); TypeScript guests need **Bun**; **Go** guests need Go + **TinyGo**
0.41+ plus `wit-bindgen-go`, **binaryen** (`wasm-opt`), and `wasm-tools`. `mise install`
pins the full Go toolchain reproducibly — see `mise.toml`.

## Commands

```sh
# Rust
cargo test                      # all tests (unit + integration)
cargo test -p rusm-metrics      # one crate
cargo fmt --check               # formatting gate
cargo llvm-cov --workspace --ignore-filename-regex 'main\.rs' --summary-only

# Dashboard (Bun, never Node)
cd bench/dashboard
bun install
bun test --coverage
bunx prettier --check src
```

## Where tests live

- **Unit tests**: inline at the bottom of each file in `#[cfg(test)] mod tests`
  (idiomatic Rust — they can reach private items, and `#[cfg(test)]` means zero
  cost in release builds).
- **Integration tests**: `<crate>/tests/*.rs` (e.g. the live-server test that
  drives the WebSocket end to end).
- **Dashboard**: pure logic in `*.ts` with `*.test.ts` beside it; presentational
  `.tsx` is excluded from the coverage gate.

## Recipe: add a host function (phase 6+)

1. Write a failing test: a `.wat`/guest module that imports `rusm::<module>::<fn>`
   and asserts the host-observable effect.
2. Define the function on the Wasmtime `Linker`, reading/writing guest memory via
   `Caller` and the per-process `ProcessState` in the `Store`.
3. Make it green; document the function in the [host ABI reference](/deep-dive/host-abi-reference).
4. Update the relevant `phases/` and `deep-dive/` doc.

## Coverage notes

`main.rs` files are thin CLI glue and excluded via `--ignore-filename-regex`.
`server.rs` and `rusm-node`'s `handle_connection` keep a few defensive async arms
(broadcast lag, peer mid-stream disconnect, accept/send failure) that only contrived
tests could reach — these are the documented exception, not a gap to paper over with
weak tests. The **decision** logic those loops drive is always extracted into
synchronous/`async` helpers that *are* unit-tested directly — e.g. `handle_text` and
`eval_line` (the attach REPL's command routing and eval gating) are covered without a
socket; only the raw `select!` over the WebSocket stays uncovered.
