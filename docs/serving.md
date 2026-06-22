# Serve over HTTP, WebSocket & SSE

Any component can be a high-throughput **HTTP / WS / SSE** server — declare a
`[[serve]]` entry and run `rusm serve`. Serving is always **ephemeral**: HTTP/SSE run
a fresh sandboxed instance per request, WS one sandboxed process per connection. A
serving instance never holds state across requests — for that, run a long-lived
`[components.<name>]` service (`resident = true`) and reach it over the actor API
(`whereis` / `call`), or persist to the node `store` (`kv`). The fastest start is `rusm new <name>`; here is
the whole shape so you can copy and adapt it.

The layout (a Rust component shown; a TS one swaps `Cargo.toml` + `src/lib.rs` for a
single `index.ts`):

```text
my-api/
├── rusm.toml
├── .env                      # optional — env vars (the real process env always wins)
├── components/
│   └── api/
│       ├── Cargo.toml        # Rust  ·  or a single index.ts (TS)
│       └── src/lib.rs
└── wasm/                     # rusm build writes api.{wasm,qjsbc,js} here
```

`rusm.toml` — one `[[serve]]` listener hosts the component on a real port; its own
`[serve.routes]` subtable maps requests to handler actions (Rust only — TS handlers
dispatch themselves). The `[[serve]]` entry is a pure listener; the handler it routes to
is a `[components.<name>]` entry that carries its own capability:

```toml
[[serve]]                    # a pure listener — no component, no capability
protocol = "http"            # http | sse | ws
listen = "127.0.0.1:8080"

[serve.routes]               # this listener's own routes
"GET /" = "api#home"               # → the `home` action in the `api` component
"GET /users/:id" = "api#show"      # :id is a path param, read from `Params`

[components.api]             # the handler the routes name → ./wasm/api.{wasm,qjsbc,js}
capability = "sandboxed"     # carries its own capability (spawned per request)
```

The handler — same job, your language.

::: code-group

```ts [components/api/index.ts]
// A web-standard TS HTTP handler — a `wasi:http` per-request component. It does
// its own dispatch, so no `[serve.routes]` table is needed.
export default function handle(request: Request): Response {
  const { pathname } = new URL(request.url);
  if (pathname === "/") return new Response("Hello from RUSM 👋\n");
  return new Response("not found\n", { status: 404 });
}
```

```rust [components/api/src/lib.rs]
// A routed Rust HTTP component: each `pub fn` is an action named in `[serve.routes]`.
// No `main`, no router — routing is declarative config. The macro hides the
// world, `Guest`, and `export!`.
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

```go [components/api/main.go]
// A routed Go HTTP component: each handler is an action named in `[serve.routes]`.
// No main, no router — routing is declarative config; the bindings live in the SDK.
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

Build and serve:

```sh
rusm build
rusm serve
# serving 1 endpoint(s):
#   api              http://127.0.0.1:8080
curl http://127.0.0.1:8080/            # Hello from RUSM 👋
curl http://127.0.0.1:8080/users/42    # user 42   (Rust, via the :id route)
```

To adapt: add more `[serve.routes]` entries and matching `pub fn`s; stream **SSE** with
`protocol = "sse"` and a `rusm_rs::sse::serve` handler (`open`/`message`/`close`, one
process per connection — the TS twin is `export default sse({ open, message, close })`,
the Go twin `web.Sse{ Open, Message, Close }.Serve()`); or serve
**WebSocket** with `protocol = "ws"` and a `rusm_rs::ws::serve` handler
(`open`/`message`/`close`) — one sandboxed process per connection (the TS twin is
`export default websocket({ open, message, close })` from `rusm-ts`; the Go twin is
`web.WebSocket{ Open, Message, Close }.Serve()`). See
[the serving model](./concepts/serving-model).
