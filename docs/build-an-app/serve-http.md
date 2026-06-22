# Serve HTTP

Let's put a component on a real HTTP port. It takes two things: a `rusm.toml` that declares
the **listener** and how it finds a handler, and the **handler** code itself. We'll serve
`GET /` and `GET /users/:id`, then run it and `curl` it.

## 1. Declare the listener

A `[[serve]]` block is a port. How it finds its handler depends on the language:

- **Rust & Go — routed.** A `[serve.routes]` table maps `"METHOD /path"` to
  `"component#action"`. One component, many routes; RUSM dispatches each request to the
  right function.
- **TypeScript — self-routing.** A TS HTTP component is one `export default { fetch }` that
  does its own routing, so it needs no routes table — just point the listener at it with
  `component = "..."`.

::: code-group

```toml [Rust / Go]
[[serve]]
protocol = "http"
listen = "127.0.0.1:8080"

[serve.routes]                       # "METHOD /path/:param" = "component#action"
"GET /" = "api#home"                 # → the `home` action
"GET /users/:id" = "api#show"        # :id is a path param, read from Params

[components.api]                     # the handler → ./wasm/api.{wasm,js}
capability = "sandboxed"             # carries its own capability (spawned per request)
```

```toml [TypeScript]
[[serve]]
protocol = "http"
listen = "127.0.0.1:8080"
component = "api"                    # one self-routing fetch handler → ./wasm/api.js

[components.api]
capability = "sandboxed"
```

:::

A path param is `:name`; a trailing `*` is a wildcard. Specificity wins (literal > param >
wildcard); a matched path with the wrong method is `405`, no match is `404`.

## 2. Write the handler

::: code-group

```ts [TypeScript]
// components/api/index.ts — a web-standard fetch handler; it routes itself.
export default function handle(request: Request): Response {
  const url = new URL(request.url);
  if (url.pathname === "/") return new Response("Hello from RUSM 👋\n");
  const m = url.pathname.match(/^\/users\/(.+)$/);
  if (m) return new Response(`user ${m[1]}\n`);
  return new Response("not found\n", { status: 404 });
}
```

```rust [Rust]
// components/api/src/lib.rs — one `pub fn` per action in [serve.routes].
// No main, no router — the macro hides the world, Guest, and export!.
use rusm_rs::http::{Params, Request, Response};

#[rusm_rs::handlers]
pub mod api {
    use super::*;
    pub fn home(_req: Request, _p: Params) -> Response {
        Response::text("Hello from RUSM 👋\n")
    }
    pub fn show(_req: Request, p: Params) -> Response {
        Response::text(format!("user {}\n", p.get("id").unwrap_or("?")))
    }
}
```

```go [Go]
// components/api/main.go — register one handler per action in [serve.routes].
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	h.Handle("home", func(_ web.Request, _ web.Params) web.Response {
		return web.Text("Hello from RUSM 👋\n")
	})
	h.Handle("show", func(_ web.Request, p web.Params) web.Response {
		id := p.Get("id")
		if id == "" {
			id = "?"
		}
		return web.Text("user " + id + "\n")
	})
	h.Serve()
}
```

:::

## 3. Build, serve, test

```sh
rusm build
rusm serve
# serving 1 endpoint(s):  api → http://127.0.0.1:8080
curl http://127.0.0.1:8080/            # Hello from RUSM 👋
curl http://127.0.0.1:8080/users/42    # user 42
```

## How it runs

Each request gets a **fresh, sandboxed instance** — spawned, it runs the action, replies, and
is gone. Nothing is shared between requests, so a crash drops *one* request and never the
server, and there's no head-of-line blocking. Because the instance is ephemeral, **don't keep
state in it** — for that, reach a resident [stateful service](/build-an-app/stateful-service)
over the actor API, or persist to the node `store` (`kv`). For the execution model in full,
see [the serving model](/deep-dive/serving-model); for every `[[serve]]`/`[serve.routes]`
field, the [configuration reference](/deep-dive/configuration).

Next: [Serve WebSocket](/build-an-app/serve-websocket) · [Serve SSE](/build-an-app/serve-sse).
