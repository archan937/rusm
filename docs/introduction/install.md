# Install

RUSM runs WebAssembly components as isolated, supervised, Erlang-style processes and serves
them over HTTP/WS/SSE. The fastest way in is to scaffold an app and serve it — start here.

First, the prerequisites. You always need Rust (the CLI installs from crates.io, and Rust
guests build on it); add Bun or TinyGo only for the guest language you write in:

- **Rust** 1.94+ via [`rustup`](https://rustup.rs) — required. To build Rust guest
  components, add the Wasm target: `rustup target add wasm32-wasip2` (and `wasm32-wasip1`
  for core modules).
- **Bun** 1.3+ ([bun.sh](https://bun.sh)) — to build **TypeScript** components; never Node.js.
- **TinyGo** 0.40+ ([tinygo.org](https://tinygo.org)) — to build **Go** components
  (`rusm build` drives it for you).

Then install the `rusm` CLI (scaffold, build, serve):

```sh
cargo install rusm-cli
```

Next: the [Quick start](/introduction/quick-start) takes you from nothing to a live server in four commands.
