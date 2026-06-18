# Collaborative todo board (TypeScript)

A complete, all-TypeScript RUSM app. Each component is an isolated, supervised WASM
process; together they show **all three serving shapes** *and* the **guest-composition**
story (services + the typed client), unified by one idea — **process-group tags as
pub/sub**, so components talk without a broker.

| Component | Kind | Showcases |
| --- | --- | --- |
| **`api`** | HTTP (`fetch`) | CRUD todos in durable `kv`; publishes each change to the feed; serves the web UI at `/` |
| **`feed`** | SSE | a per-connection stream of the live todo list — pushed on every change, never polled |
| **`chat`** | WebSocket | rooms, one process per connection, fan-out over tags |
| **`store`** | service | the todo data as a service — the typed-client target (`call` / streaming / `cast`) |
| **`reporter`** | worker (resident) | drives `store` through the typed client: a call, a callback, a streamed result, a cast |

## Run it

```sh
bun install        # once, to resolve the local rusm-ts
rusm build         # compile components/ -> wasm/
rusm serve         # http :8080  ·  sse :8081  ·  ws :8082
```

Then **open <http://localhost:8080>** — the web UI explains each part and is fully
interactive (add/toggle/delete todos, chat). The resident `reporter` seeds a welcome list
on first boot (watch `rusm serve`'s log for the composition demo).

## Exploit it from the shell

```sh
# HTTP CRUD (the `api`) — same data the web UI uses
curl localhost:8080/todos                                   # list
curl -X POST localhost:8080/todos -d '{"text":"buy milk"}'  # add  -> {"id":N,...}
curl -X PATCH  localhost:8080/todos/1                        # toggle done
curl -X DELETE localhost:8080/todos/1                        # remove

# Live feed (the `feed`) — every change above streams here in real time
curl -N localhost:8081
#   data: [ ...the todo list, re-sent on each change... ]

# Chat (the `chat`) — needs a WebSocket client (or use the web UI). With websocat:
#   websocat ws://localhost:8082
#   > {"join":"general"}      # join a room
#   > {"say":"hello"}         # fan out to everyone in #general
```

## How it fits together

- **`api`** is a web-standard `fetch` handler (it does its own routing — HTTP needs no
  `[serve.routes]` for TS). It reads/writes the todo list in `kv` and, on every change,
  **publishes** the new list to the `todos` process-group tag (`whereisTag` + `send`).
- **`feed`** is one SSE process per client. On connect it emits the current list; each
  published change then lands in its mailbox and streams straight out — true push.
- **`chat`** is one process per WebSocket connection. A room is a tag (`room:<name>`);
  joining tags the connection and a message fans out to the room's members. A peer's
  message arrives in the same mailbox, so the handler tells client input from peer relays
  by the wire shape (`{join}`/`{say}` vs `{from,text}`).
- **`store` + `reporter`** are the composition half. `store` is a service — its exported
  functions *are* the API; `reporter` reaches it with the concealed typed client
  `spawn<Store>("store")` and exercises a `call` (`await store.list()`), a **callback**
  (`store.importMany(texts, onProgress)`), a **streamed** result (`for await … of
  store.all()`), and a **cast** (`store.cast.ping()`). It then parks (a resident worker
  never just exits, or the supervisor would restart it).

The shared todo model is [`lib/todos.ts`](./lib/todos.ts) — the single source the `api`
and the `store` service both build on.

## Platform vs. application

Everything here is **application** code. The platform (`rusm-*`) provides only generic
primitives — `kv`, process-group tags, the typed client, the HTTP/SSE/WS serving — and
never knows what a "todo", a "room", or a "feed" is.

## Layout

- `components/{api,feed,chat,store,reporter}/index.ts` — the components (+ `*.test.ts`).
- `lib/todos.ts` — the shared todo data layer; `lib/page.ts` — the web UI served at `/`.
- `rusm.toml` — listeners, capability profiles, the resident `reporter`.

## Test

```sh
bun test           # unit tests for each component (23)
```

Capabilities are least-privilege: `api`/`feed`/`store` get `storage`; `reporter` gets
`spawn`; `chat` is fully sandboxed.
