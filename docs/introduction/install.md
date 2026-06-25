# Install

RUSM runs WebAssembly components as isolated, supervised, Erlang-style processes and serves
them over HTTP/WS/SSE. You don't clone anything to use it — install one CLI and scaffold an
app. (Clone the repo only to hack on RUSM itself or run its live dashboard.)

## Prerequisites

You always need Rust — the CLI installs from crates.io, and Rust guests build on it. Add Bun
or TinyGo only for the guest language you actually write in:

- **Rust** 1.94+ via [`rustup`](https://rustup.rs) — required. For Rust guest components,
  add the Wasm target: `rustup target add wasm32-wasip2` (and `wasm32-wasip1` for core modules).
- **Bun** 1.3+ ([bun.sh](https://bun.sh)) — to build **TypeScript** components; never Node.js.
- **TinyGo** 0.41+ ([tinygo.org](https://tinygo.org)) — to build **Go** components;
  `rusm build` drives it for you.

## Install the CLI

```sh
cargo install rusm-cli            # the `rusm` command
rustup target add wasm32-wasip2   # only if you'll write Rust guests
```

Verify it:

```sh
rusm --version
```

Next: the [Quick start](/introduction/quick-start) takes you from nothing to a live server in
four commands.
