# Collaborative todo board (Rust)

The Rust twin of the [TypeScript example](../typescript) — the same collaborative todo
board, idiomatic to Rust. Five components, each an isolated, supervised WASM process,
unified by **process-group tags as pub/sub** (no broker).

| Component | Kind | Shape |
| --- | --- | --- |
| **`api`** | HTTP | `#[rusm_rs::handlers]` actions, routed declaratively in `rusm.toml`; serves the web UI at `/` |
| **`feed`** | SSE | a per-connection `sse::serve` handler |
| **`chat`** | WebSocket | a per-connection `ws::serve` handler |
| **`store`** | service | a `#[rusm_rs::service]` module — its exported fns ARE the API |
| **`reporter`** | worker | resident; drives `store` through the generated typed client |

## Run it

Requires the `wasm32-wasip2` target (`rustup target add wasm32-wasip2` once):

```sh
rusm build         # compile components/ → wasm/ (cargo, wasm32-wasip2)
rusm serve         # http :8080  ·  sse :8081  ·  ws :8082
```

Then **open <http://localhost:8080>** — the web UI explains each part and is fully
interactive (add/toggle/delete todos, chat).

```sh
curl localhost:8080/todos                                   # the list (seeded by reporter)
curl -X POST localhost:8080/todos -d '{"text":"buy milk"}'  # {"id":...}
curl -N localhost:8081                                       # live SSE feed of the list
# WebSocket chat on :8082 — send {"join":"general"} then {"say":"hi"}
```

The first `rusm serve` boots the resident **`reporter`**, which seeds a short welcome list
(only when the board is empty) — so the page has content on first open.

## Serving — three shapes

- **`api`** is a module of `#[rusm_rs::handlers]` actions (`fn(Request, Params) ->
  Response`); routing is **declarative** in `rusm.toml`'s `[serve.routes]` (Rust handlers
  don't self-route, unlike the TS `fetch`). It reads/writes the todo list in `kv`,
  **publishes** the new list to the `todos` process-group tag on every change, and serves
  the explanatory web page at `/`.
- **`feed`** implements `sse::Handler` (`open`/`message`/`close`) and `sse::serve`s it. On
  connect it subscribes to the tag and emits the current list; each published change then
  streams straight out — true push, no polling.
- **`chat`** implements `ws::Handler`. A room is a tag (`room:<name>`); a message fans out
  to the room's members. The same `open`/`message`/`close` lifecycle as the TS and Go
  handlers — the handler shape resembles across all three languages.

## Composition — service + typed client

The `store` and `reporter` show **guest composition** over the same todos:

- **`store`** is a `#[rusm_rs::service]` module: its exported functions ARE the API, and
  `store::serve()` runs the receive→dispatch→reply loop around them. It exposes a plain
  call (`list`/`add`/`toggle`/`remove`), a **streamed** result (`all` → `impl Iterator`), a
  **callback** argument (`import_many`), and a cast-friendly `ping`.
- **`reporter`** is a resident worker: it `store::Client::spawn("store")`s the service and
  exercises every shape — a call, a callback (seeding the welcome list with progress
  reported back), a streamed read, and a fire-and-forget cast — then **parks** (a resident
  worker loops or parks; returning would let the supervisor restart it in a loop).

Because the service and its `Client` are both generated from the one `store-svc` module,
they can never drift. (The HTTP `api` mutates the todos directly: a per-request instance
has no mailbox to host a client — composition lives in the actor-side `store`/`reporter`.)

## Platform vs. application

Everything here is **application** code. The platform (`rusm-*`) provides only generic
primitives — `kv`, process-group tags, the HTTP/SSE/WS serving, the service/typed-client
machinery — and never knows what a "todo", a "room", or a "feed" is.

## Layout

- `components/{api,feed,chat,store,reporter}/` — the component crates (cargo, `wasm32-wasip2`).
- `todos/` — the shared todo data layer (the single source the `api`, `feed`, and `store` share).
- `store-svc/` — the shared `#[rusm_rs::service]` module (one source for `store::serve()` and `store::Client`).
- `rusm.toml` — listeners, routes, response headers, capability profiles.
- `wasm/` — built artifacts (git-ignored), produced by `rusm build`.

Capabilities are least-privilege: `api`/`feed`/`store` get `storage`; `reporter` gets
`spawn`; `chat` is fully sandboxed. The cross-origin live feed embedded in the page is
enabled by the feed listener's `[serve.headers]` CORS policy (the app's policy, applied by
the platform).
