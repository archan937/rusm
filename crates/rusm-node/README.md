# rusm-node

> The RUSM node layer — the `rusm.toml` manifest, capability profiles, declarative routing, and the attach protocol.

`rusm-node` turns a bare runtime into a configured, attachable **node** for
[RUSM](https://github.com/archan937/rusm). It owns the operator-facing surface that the
`rusm` CLI drives:

- **The manifest** — parsing `rusm.toml`: `[node]` settings, `[[serve]]` listeners with a
  declarative `[serve.routes]` table, `[components.<name>]` services (resident or per-call),
  `[capabilities.*]` profiles, the embedded `store`, and `[log]` gating.
- **Capability profiles** — resolving a component's declared, default-deny grants into the
  `WasiCtx` + limits the sandbox enforces.
- **Route compilation** — a `RouteTable` (`"METHOD /path/:param" = "component#action"`,
  specificity-ordered) bridged into the routing-agnostic serving layer.
- **The attach protocol** — the wire a remote observer/REPL uses to inspect a live node.

It composes [`rusm-otp`](https://crates.io/crates/rusm-otp) and
[`rusm-wasm`](https://crates.io/crates/rusm-wasm); the `rusm` CLI
([`rusm-cli`](https://crates.io/crates/rusm-cli)) is the usual entry point.

Part of [RUSM](https://github.com/archan937/rusm). See the
[repo README](https://github.com/archan937/rusm#readme) and the
[CLI reference](https://github.com/archan937/rusm/blob/main/docs/build-an-app/the-rusm-cli.md).
