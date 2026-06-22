# rusm-cli

> The `rusm` command-line tool — scaffold, build, run, and serve RUSM apps, and attach to live nodes.

`rusm-cli` installs the **`rusm`** binary: the developer-facing entry point to
[RUSM](https://github.com/archan937/rusm), an Erlang-inspired WebAssembly runtime in Rust
(isolated lightweight processes, message passing, supervision, per-actor sandboxing, and
"write blocking code → the runtime makes it async").

## Install

```sh
cargo install rusm-cli      # installs the `rusm` binary
```

## Commands

```sh
rusm new <name>     # scaffold a ready-to-serve app (a TS HTTP component + rusm.toml)
rusm build          # compile each component under ./components to ./wasm
rusm run            # run the app's components under their declared capabilities
rusm dev            # watch ./components and rebuild + reload on edit
rusm serve          # host rusm.toml [[serve]] entries (HTTP / WS / SSE) on real ports
rusm node start     # host the app as an attachable node
rusm attach         # observe a running node — local or host[:port]
```

A new app is fully working end-to-end:

```sh
rusm new hello && cd hello && rusm build && rusm serve
curl http://127.0.0.1:8080/
```

## The app model

An app is described by **`rusm.toml`**: `[[serve]]` listeners (with declarative
`[serve.routes]`), `[components.<name>]` services (resident or per-call), `[capabilities.*]`
profiles (default-deny), and an embedded `store`. Components are written in **Rust**
(via the [`rusm-rs`](https://crates.io/crates/rusm-rs) crate), **TypeScript** (via the
[`rusm-ts`](https://www.npmjs.com/package/rusm-ts) npm package), or **Go** (via the
[`rusm-go`](https://github.com/archan937/rusm/tree/main/packages/rusm-go) SDK, compiled
with TinyGo) — all three share one wire and interoperate.

Part of [RUSM](https://github.com/archan937/rusm). See the
[repo README](https://github.com/archan937/rusm#readme), the
[`rusm` CLI reference](https://github.com/archan937/rusm/blob/main/docs/build-an-app/the-rusm-cli.md),
and the [Getting Started guide](https://github.com/archan937/rusm/blob/main/docs/introduction/quick-start.md).
