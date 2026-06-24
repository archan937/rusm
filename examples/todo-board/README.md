# todo-board

A collaborative todo board in each guest language — the same app (HTTP CRUD, live SSE feed,
WebSocket chat, a service, a resident worker) built three times to show what idiomatic RUSM
looks like in **TypeScript**, **Rust**, and **Go**.

Each variant runs five isolated, supervised WASM processes, unified by **process-group tags as
pub/sub** (no broker). All three serve on the same ports and expose the same web UI.

| Flavour | Language | Toolchain |
|---|---|---|
| [`typescript/`](./typescript/) | TypeScript (Bun) | `rusm build` bundles via Bun |
| [`rust/`](./rust/) | Rust | `cargo` → `wasm32-wasip2` |
| [`go/`](./go/) | Go | TinyGo → `wasm32-wasip2` |

## Run any flavour

```sh
cd examples/todo-board/<typescript|rust|go>
rusm build && rusm serve
```

Then open <http://localhost:8080>.

## The five components

| Component | Kind | Role |
|---|---|---|
| **`api`** | HTTP | CRUD todos in durable `kv`; publishes each change via tags; serves the web UI |
| **`feed`** | SSE | one process per client — pushed live todo list on every change |
| **`chat`** | WebSocket | one process per connection — rooms modelled as process-group tags |
| **`store`** | service | the todo data as a typed service (call / stream / cast) |
| **`reporter`** | worker (resident) | drives `store` on boot: exercises every composition shape, seeds the welcome list |

## What it showcases

- All three serving shapes in one app (HTTP, SSE, WebSocket)
- **Guest composition** — the `store` service + generated typed `Client`
- **Process-group tags** as pub/sub (`subscribe` = tag yourself, `publish` = `whereisTag` + `send`)
- Least-privilege capability profiles
- Per-request / per-connection process isolation (a crash drops one unit, not the server)
