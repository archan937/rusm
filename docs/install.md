# Install

RUSM runs WebAssembly components as isolated, supervised, Erlang-style processes and serves
them over HTTP/WS/SSE. The fastest way in is to scaffold an app and serve it — start here.

First, the prerequisites:

- **Rust** 1.94+ via [`rustup`](https://rustup.rs). To build guest components, add the
  Wasm target: `rustup target add wasm32-wasip2` (and `wasm32-wasip1` for core modules).
- **Bun** 1.3+ ([bun.sh](https://bun.sh)) — to build TypeScript components; never Node.js.

Then install the `rusm` CLI (scaffold, build, serve):

```sh
cargo install rusm-cli
```

Next: the [Quick start](./quick-start) takes you from nothing to a live server in four commands.
