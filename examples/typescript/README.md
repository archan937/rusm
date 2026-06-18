# Collaborative todo board (TypeScript)

A small but complete RUSM app showing all three serving shapes in TypeScript, each as an
isolated, supervised WASM process — and the **one** idea that ties them together:
**process-group tags as pub/sub**, so components talk without a broker.

| Component | Protocol | What it does |
| --- | --- | --- |
| **`api`** | HTTP | CRUD todos in durable `kv`; publishes every change to subscribers |
| **`feed`** | SSE | a live stream of the todo list — pushed on every change, never polled |
| **`chat`** | WebSocket | rooms, one process per connection, fan-out over tags |

```
rusm build && rusm serve
# then, in another terminal:
curl localhost:8080/todos                                   # []
curl -X POST localhost:8080/todos -d '{"text":"buy milk"}'  # {"id":1,...}
curl -N localhost:8081                                       # live SSE feed of the list
# WebSocket chat on :8082 — send {"join":"general"} then {"say":"hi"}
```

## How it fits together

- **`api`** is a web-standard `fetch` handler (it does its own routing — HTTP needs no
  `[serve.routes]` for TS). Each request is a fresh, sandboxed instance. It reads/writes
  the todo list in `kv` and, on every change, **publishes** the new list to the `todos`
  process-group tag (`whereisTag` + `send`).
- **`feed`** is one SSE process per connected client. On connect it subscribes to the
  `todos` tag and emits the current list; thereafter each change the `api` publishes lands
  in its mailbox and is streamed straight out — **true push, no polling**. The
  subscription releases automatically when the client disconnects.
- **`chat`** is one process per WebSocket connection. A room is a tag (`room:<name>`):
  joining tags the connection, and a message fans out to the room's members. A peer's
  message arrives in the same mailbox, so the handler tells client input from peer relays
  by the wire shape (`{join}`/`{say}` vs `{from,text}`).

The shared todo model lives in [`lib/todos.ts`](./lib/todos.ts) — the single source the
`api` mutates and the `feed` reads.

## Platform vs. application

Everything here is **application** code. The platform (`rusm-*`) provides only generic
primitives — `kv`, process-group tags, and the HTTP/SSE/WS serving — and never knows what
a "todo", a "room", or a "feed" is. That split is deliberate: the same tags that power the
chat rooms power the feed's pub/sub, and nothing app-specific leaks into the runtime.

## Layout

- `components/{api,feed,chat}/index.ts` — the three handlers (+ their `*.test.ts`).
- `lib/todos.ts` — the shared todo data layer.
- `rusm.toml` — what to serve, on which ports, under which capability profile.
- `wasm/` — built artifacts (git-ignored), produced by `rusm build`.

## Test

```sh
bun install
bun test          # unit tests for each component's logic
```

Capabilities are least-privilege: `api`/`feed` get `storage` (the todo list); `chat` is
fully sandboxed (tags only).
