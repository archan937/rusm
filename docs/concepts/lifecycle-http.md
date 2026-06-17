# Lifecycle — HTTP component

A per-request HTTP handler: a `#[rusm_rs::handlers]` action (Rust), a `web.Handlers`
action (Go), or a `wasi:http` `export default { fetch }` (TypeScript). **A fresh
sandboxed instance per request**,
which handles exactly one request and exits. See the
[overview](./component-lifecycle.md) for the shared two-domain model and failure
vocabulary this chapter builds on.

## Shape (what you write)

::: code-group

```rust [Rust]
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;
    // routed from `[serve.routes]`:  "GET /users/:id" = "api#show"
    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}", p.get("id").unwrap_or("?")))
    }
}
```

```ts [TypeScript]
// A wasi:http component — instantiated per request. It dispatches itself, so it
// needs no `[serve.routes]` table; read path params off the URL.
export default function handle(request: Request): Response {
  const id = new URL(request.url).pathname.split("/").pop();
  return new Response(`user ${id ?? "?"}`, {
    headers: { "content-type": "text/plain" },
  });
}
```

```go [Go]
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	// routed from `[serve.routes]`:  "GET /users/:id" = "api#show"
	h.Handle("show", func(_ web.Request, p web.Params) web.Response {
		id := p.Get("id")
		if id == "" {
			id = "?"
		}
		return web.Text("user " + id)
	})
	h.Serve()
}
```

:::

That's the **application domain** in full — a function from a request to a response. No
`main`, no router, no request/response wire. (The Rust `#[handlers]` and Go
`web.Handlers` forms are routed by `[serve.routes]` to a named action; the TypeScript
`export default` form is a `wasi:http` component that dispatches itself.)

## Platform owns / you write

- **Platform owns:** accepting the connection, parsing HTTP, resolving the route from
  this listener's `[serve.routes]`, spawning the handler fresh, sending the request over
  the actor wire, the ephemeral reply *responder*, writing the HTTP response, and
  reclaiming the instance.
- **You write:** `fn action(Request, Params) -> Response`.

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Normal** | route → spawn fresh → dispatch → build the HTTP response → reclaim the instance | the action returns a `Response` | the response; the instance is gone |
| **Error status** (e.g. `Response::new(400, …)`) | writes it verbatim | chose to return it | that status — **not** a failure |
| **No route / wrong method** | answers **404** / **405** | (never spawned) | 404 / 405 |
| **Unregistered component** in a route | answers **500** | (never spawned) | 500 — a manifest error |
| **Connection error** (unreadable body, malformed request line) | answers **400**, or drops the malformed connection, *before* dispatch | (usually never spawned) | 400 / dropped |
| **Client disconnect** before the reply | the response write fails; the connection is dropped | the action still ran to completion — it can't tell | nothing sent; **no crash** |
| **Crash (trap)** | the responder's reply channel drops → answers **502** → the process is Crashed and reclaimed | the `panic!` / `.unwrap()` / `unreachable` | **502**; only this request affected |
| **Memory crash (OOM)** | the `StoreLimiter` cap trips a trap → handled like any crash → **502** | exceeded `max-memory-mb` | **502**; the instance is discarded |

## Notes

- **Not supervised — and that's correct.** There is nothing to restart: the next
  request gets a brand-new instance regardless. A crash or OOM is contained to one
  request; the listener and every other in-flight request are untouched (each is its
  own instance with its own memory).
- **Where state goes.** A handler is stateless and disposable. Cross-request state
  lives in a [service component](./lifecycle-service.md) (reached via `whereis` +
  `call`) or in durable `kv` — never in the handler.
- **Same lifecycle, every language.** The Rust, TypeScript, and Go forms above share
  the exact per-request lifecycle in the table — a fresh instance, one request, then exit.

Next: [SSE component](./lifecycle-sse.md) · [WebSocket component](./lifecycle-websocket.md)
