# Collaborative todo board (Rust)

The Rust twin of the [TypeScript example](../typescript) — the same collaborative todo
board, idiomatic to Rust. Three serving components, each an isolated, supervised WASM
process, unified by **process-group tags as pub/sub** (no broker).

| Component | Protocol | Shape |
| --- | --- | --- |
| **`api`** | HTTP | `#[rusm_rs::handlers]` actions, routed declaratively in `rusm.toml` |
| **`feed`** | SSE | a per-connection `sse::serve` handler |
| **`chat`** | WebSocket | a per-connection `ws::serve` handler |

```sh
rusm build && rusm serve
# then, in another terminal:
curl localhost:8080/todos                                   # []
curl -X POST localhost:8080/todos -d '{"text":"buy milk"}'  # {"id":1,...}
curl -N localhost:8081                                       # live SSE feed of the list
# WebSocket chat on :8082 — send {"join":"general"} then {"say":"hi"}
```

## How it differs from the TypeScript app (idiomatic Rust)

Same behaviour, idiomatic per language:

- **`api`** is a module of `#[rusm_rs::handlers]` actions (`fn(Request, Params) ->
  Response`); routing is **declarative** in `rusm.toml`'s `[serve.routes]` (Rust handlers
  don't self-route, unlike the TS `fetch`). It reads/writes the todo list in `kv` and
  **publishes** the new list to the `todos` process-group tag on every change.
- **`feed`** implements `sse::Handler` (`open`/`message`/`close`) and `sse::serve`s it. On
  connect it subscribes to the tag and emits the current list; each published change then
  streams straight out — true push, no polling.
- **`chat`** implements `ws::Handler`. A room is a tag (`room:<name>`); a message fans out
  to the room's members. The same `open`/`message`/`close` lifecycle as the TS and Go
  handlers — the handler shape resembles across all three languages.

The todo model lives in the shared [`todos`](./todos) crate — the single source the `api`
mutates and the `feed` reads (the Rust analogue of the TS `lib/todos.ts`).

## Platform vs. application

Everything here is **application** code. The platform (`rusm-*`) provides only generic
primitives — `kv`, process-group tags, the HTTP/SSE/WS serving — and never knows what a
"todo", a "room", or a "feed" is.

## Layout

- `components/{api,feed,chat}/` — the three handler crates (cargo, `wasm32-wasip2`).
- `todos/` — the shared todo data layer.
- `rusm.toml` — listeners, routes, capability profiles.
- `wasm/` — built artifacts (git-ignored), produced by `rusm build`.

Capabilities are least-privilege: `api`/`feed` get `storage`; `chat` is fully sandboxed.
