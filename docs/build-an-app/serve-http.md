# Serve HTTP

Let's put a component on a real HTTP port. It takes two things: a `rusm.toml` that declares
the **listener** and its routes, and the **handler** code itself. We'll serve
`GET /` and `GET /users/:id`, then run it and `curl` it.

## 1. Declare the listener

A `[[serve]]` block is a port. A `[serve.routes]` table maps `"METHOD /path"` to
`"component#action"` — one entry per route, any number of components, all in config.
No router code. No middleware. RUSM dispatches each request to the right function and
spawns a **fresh sandboxed instance** for it.

```toml
[[serve]]
protocol = "http"
listen = "127.0.0.1:8080"

[serve.routes]                       # "METHOD /path/:param" = "component#action"
"GET /"           = "api#home"       # → the `home` action in the `api` component
"GET /users/:id"  = "users#show"     # routed to a different component entirely
"POST /users"     = "users#create"   # same component, different action
"POST /webhooks"  = "hooks#receive"  # yet another component — one table, any mix

[components.api]
capability = "sandboxed"

[components.users]
capability = "sandboxed"

[components.hooks]
capability = "network-client"        # components carry their own capability profile
```

A path param is `:name`; a trailing `*` is a wildcard. Specificity wins (literal > param >
wildcard); a matched path with the wrong method is `405`, no match is `404`.

::: tip TypeScript — `export default { fetch }` also works
If you have a single self-contained TS component that does its own routing, you can skip
the routes table and point the listener at it directly with `component = "api"`. It's the
right call for a simple catch-all handler, but `[serve.routes]` is the preferred path for
anything real — it's declarative, language-agnostic, and trivially extended.
:::

## 2. Write the handler

::: code-group

```ts [TypeScript]
// components/api/index.ts — one export per action in [serve.routes].
import { type Params } from "rusm-ts";

export function home(_req: Request, _p: Params): Response {
  return new Response("Hello from RUSM 👋\n");
}
export function show(_req: Request, p: Params): Response {
  return new Response(`user ${p.id}\n`);
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

## Routing — the full picture

`[serve.routes]` is a single declarative table that wires your whole API. One listener,
any number of routes, any number of components — no router code to write, no middleware
to wire up.

```toml
[[serve]]
protocol = "http"
listen   = "0.0.0.0:8080"

[serve.routes]
# static routes — exact match, fastest
"GET  /"              = "api#home"
"GET  /healthz"       = "api#health"

# path parameters — :name captures one segment, read from Params
"GET  /users/:id"     = "users#show"
"POST /users/:id"     = "users#update"

# wildcard — trailing * captures the rest of the path
"GET  /static/*"      = "assets#serve"

# different components on the same listener — no problem
"POST /webhooks/gh"   = "github#event"
"GET  /admin/*"       = "admin#handle"
```

**Specificity wins**: a literal beats a param segment beats a wildcard — `/users/me`
matches `"GET /users/me"` before `"GET /users/:id"`. A matched path with the wrong
method is `405 Method Not Allowed`; no match at all is `404 Not Found`. The component
named in `component#action` is spawned **fresh per request** — so a crash drops one
request, never the server, and there is no head-of-line blocking.

**Multiple listeners** get their own `[serve.routes]` table — public API on 8080, admin
on 9090, each with its own routes and components:

```toml
[[serve]]
protocol = "http"
listen   = "0.0.0.0:8080"

[serve.routes]
"GET /users/:id" = "api#show"
"POST /users"    = "api#create"

[[serve]]
protocol = "http"
listen   = "127.0.0.1:9090"

[serve.routes]
"GET  /metrics" = "admin#metrics"
"POST /reload"  = "admin#reload"
```

No port shares a routes table with another — listeners are fully independent.

## How it runs

Each request gets a **fresh, sandboxed instance** — spawned, it runs the action, replies, and
is gone. Nothing is shared between requests, so a crash drops *one* request and never the
server, and there's no head-of-line blocking. Because the instance is ephemeral, **don't keep
state in it** — for that, reach a resident [stateful service](/build-an-app/build-a-stateful-service)
over the actor API, or persist to the node `store` (`kv`). For the execution model in full,
see [the serving model](/deep-dive/the-serving-model); for every `[[serve]]`/`[serve.routes]`
field, the [configuration reference](/deep-dive/configuration).

Next: [Serve WebSocket](/build-an-app/serve-websocket) · [Serve SSE](/build-an-app/serve-sse).
