# Collaborative todo board (Go)

The Go twin of the [TypeScript](../typescript) and [Rust](../rust) examples — the same
collaborative todo board, idiomatic to Go (TinyGo → `wasm32-wasip2`). Five components, each
an isolated, supervised WASM process, unified by **process-group tags as pub/sub** (no broker).

| Component | Kind | Shape |
| --- | --- | --- |
| **`api`** | HTTP | `web.NewHandlers()` actions, routed declaratively in `rusm.toml`; serves the web UI at `/` |
| **`feed`** | SSE | a per-connection `web.Sse{Open,Message,Close}` handler |
| **`chat`** | WebSocket | a per-connection `web.WebSocket{Open,Message,Close}` handler |
| **`store`** | service | `rusm.NewService()` — its registered ops ARE the API |
| **`reporter`** | worker | resident; drives `store` through the typed `store.Client` |

## Run it

Requires **Go** and **TinyGo** installed (`rusm build` drives `go mod tidy` + `tinygo build`):

```sh
rusm build         # compile components/ → wasm/ (TinyGo, wasm32-wasip2)
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

- **`api`** registers buffered actions with `web.NewHandlers()`; routing is **declarative**
  in `rusm.toml`'s `[serve.routes]`, and the host spawns a fresh instance per request. It
  reads/writes the todo list in `kv`, **publishes** the new list to the `todos`
  process-group tag on every change, and serves the explanatory web page (a `//go:embed`)
  at `/`.
- **`feed`** is a `web.Sse{Open, Message, Close}` handler. `Open` subscribes to the tag and
  emits the current list; each published change then streams straight out — true push, no
  polling.
- **`chat`** is a `web.WebSocket{Open, Message, Close}` handler. A room is a tag
  (`room:<name>`); a message fans out to the room's members. The same `Open`/`Message`/
  `Close` lifecycle as the TS and Rust handlers — the handler shape resembles across all
  three languages.

## Composition — service + typed client

The `store` and `reporter` show **guest composition** over the same todos:

- **`store`** runs `store.Serve()`: a `rusm.NewService()` whose handlers ARE the API. It
  exposes plain calls (`list`/`add`/`toggle`/`remove`), a **streamed** result (`all` via
  `HandleStream`), a **callback** argument (`import`), and a cast-friendly `ping`.
- **`reporter`** is a resident worker: it `store.Spawn()`s the service and exercises every
  shape through the typed client — a call, a callback (seeding the welcome list with
  progress reported back via `rusm.CB`), a streamed read (`for range client.All()`), and a
  fire-and-forget cast — then **parks** (a resident worker loops or parks; returning would
  let the supervisor restart it in a loop).

The service registration and the typed `Client` are defined together in the shared
`todoboard/store` package — sharing the operation-name constants, so the two halves can't
drift. (The HTTP `api` mutates the todos directly: a per-request instance has no mailbox to
host a client — composition lives in the actor-side `store`/`reporter`.)

## Platform vs. application

Everything here is **application** code. The platform (`rusm-go`) provides only generic
primitives — `kv`, process-group tags, the HTTP/SSE/WS serving, the service/typed-client
machinery — and never knows what a "todo", a "room", or a "feed" is. The Go SDK routes the
standard library `log`/`log/slog` to the host logger, so the components log the normal Go way.

## Layout

- `components/{api,feed,chat,store,reporter}/` — the component modules (TinyGo, `wasm32-wasip2`).
- `shared/` — one module (`todoboard`) with two packages: `todos` (the data layer) and
  `store` (the service contract). Components import it via a local `replace`.
- `rusm.toml` — listeners, routes, response headers, capability profiles.
- `wasm/` — built artifacts (git-ignored), produced by `rusm build`.

Capabilities are least-privilege: `api`/`feed`/`store` get `storage`; `reporter` gets
`spawn`; `chat` is fully sandboxed. The cross-origin live feed embedded in the page is
enabled by the feed listener's `[serve.headers]` CORS policy (the app's policy, applied by
the platform).
