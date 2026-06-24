# weather-api

Three standalone apps demonstrating the same `weather` bridge in each bridge **host language**:

| Flavour | Bridge host | Guest | Serves |
|---|---|---|---|
| [`rust/`](./rust/) | `host.rs` — Rust impl, native ABI | Rust HTTP handler | HTTP :8080 |
| [`typescript/`](./typescript/) | `host.ts` — TS impl, actor round-trip | TS WebSocket handler | WS :8080 |
| [`go/`](./go/) | `host.go` — Go impl, actor round-trip | Go HTTP handler | HTTP :8080 |

The bridge contract (`bridges/weather/bridge.wit`) is identical across all three. Each app
scaffolds a `weather` bridge that any guest — TypeScript, Rust, or Go — calls as a plain
typed import.

## Run any flavour

```sh
cd examples/weather-api/<rust|typescript|go>
rusm build && rusm serve
```

## Scaffold your own

```sh
rusm new forecast --template weather --lang rs   # Rust bridge + Rust guest
rusm new forecast --template weather --lang ts   # Rust bridge + TS guest
rusm new forecast --template weather --lang go   # Rust bridge + Go guest
```

Or start with a TS or Go bridge host directly using `--bridges` and adding a `host.ts` / `host.go`.
