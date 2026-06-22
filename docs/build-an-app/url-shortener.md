# A URL shortener

The fastest way to learn the shape of a RUSM app is to build a real one. So we'll build a
working **URL shortener** — `POST` a long URL and get back a short code; visit the code and
get redirected — and meet the anatomy *every* RUSM app shares along the way: a `rusm.toml`
manifest plus components, which the `rusm` CLI builds, serves, and supervises.

It needs just two things:

- a **handler** that answers each HTTP request — `POST /shorten` and `GET /:code` — and
- somewhere to **keep the `code → URL` map** between requests.

A code-to-URL map is the textbook job for a key-value store, so we'll keep it in RUSM's
durable **`kv`**. That choice matters: the handler is spawned *fresh per request* and keeps
nothing of its own, so the shared, surviving state lives in `kv`, not in the handler. Let's
build it.

## The shape of an app

```
shortener/
├── rusm.toml          # the manifest — what the app is made of
├── components/
│   └── api/           # the HTTP handler  (TS index.ts · Rust src/lib.rs · Go main.go)
└── wasm/              # rusm build writes the compiled component here
```

One folder per component under `components/`, one `[components.<name>]` entry per component
in the manifest. You write the source; `rusm build` produces `./wasm/api.{wasm,js}`.

## The manifest

Everything that makes our shortener an app lives in one `rusm.toml`. The only difference
between languages is how routing is wired: **Rust and Go route declaratively** in a
`[serve.routes]` table; a **TypeScript** HTTP handler self-routes (one `fetch`), so it just
names the component.

::: code-group

```toml [TypeScript]
# rusm.toml
[node]
store = "links.redb"               # durable kv — the code → URL map lives here

[[serve]]                          # a listener on a real TCP port
protocol  = "http"
listen    = "127.0.0.1:8080"
component = "api"                  # one self-routing fetch handler → ./wasm/api.js

[capabilities.stateful]            # a profile that may touch durable storage
inherits      = "sandboxed"        # default-deny base
allow-storage = true               # + the kv store

[components.api]
capability = "stateful"            # the handler runs under that profile
```

```toml [Rust / Go]
# rusm.toml
[node]
store = "links.redb"               # durable kv — the code → URL map lives here

[[serve]]                          # a listener on a real TCP port
protocol = "http"
listen   = "127.0.0.1:8080"

[serve.routes]                     # map each request to a handler action
"POST /shorten" = "api#shorten"    # save a URL, return a short code
"GET  /:code"   = "api#expand"     # look the code up, redirect to the URL

[capabilities.stateful]            # a profile that may touch durable storage
inherits      = "sandboxed"        # default-deny base
allow-storage = true               # + the kv store

[components.api]
capability = "stateful"            # the handler runs under that profile
```

:::

Three ideas, and they're the whole app model:

- **`[[serve]]`** is a pure listener — a protocol and a port. For Rust/Go its **`[serve.routes]`**
  map each request (`"METHOD /path"`) to a `"component#action"`; a TS HTTP component self-routes,
  so the listener just points at it with `component`.
- **`[components.api]`** declares the handler and the **capability** it runs under. Ours needs
  `allow-storage`, because `kv` is default-deny like everything else — so we define a small
  `stateful` profile that grants it. (Capabilities are their own topic:
  [Grant capabilities](/build-an-app/grant-capabilities).)
- **`[node] store`** names the on-disk file the durable `kv` lives in. It's created on first
  write, and it's why a shortened link survives a restart.

## The handler

Now the code — the two actions, `shorten` and `expand`. Each opens the `links` bucket, and
that's the entire shortener: write a URL under a fresh code, then redirect a code back to its
URL. (We mint codes with a simple counter; a real shortener would hash or randomize.)

::: code-group

```ts [TypeScript]
// components/api/index.ts — a self-routing fetch handler over durable kv.
import { kv } from "rusm-ts";

const links = () => kv.bucket("links");

export default async function handle(req: Request): Promise<Response> {
  const url = new URL(req.url);

  // POST /shorten — the body is the long URL; store it under a fresh code.
  if (req.method === "POST" && url.pathname === "/shorten") {
    const target = (await req.text()).trim();
    if (!target) return new Response("send a URL in the body\n", { status: 400 });
    const code = String(links().list().length + 1); // a simple sequential code
    links().set(code, target);
    return new Response(`/${code}\n`, { status: 201 });
  }

  // GET /:code — look the code up and redirect to the URL.
  const stored = links().get(url.pathname.slice(1));
  if (stored) {
    const location = new TextDecoder().decode(stored);
    return new Response(null, { status: 302, headers: { location } });
  }

  return new Response("not found\n", { status: 404 });
}
```

```rust [Rust]
// components/api/src/lib.rs — routed actions over durable kv.
use rusm_rs::http::{Params, Request, Response};
use rusm_rs::kv;

#[rusm_rs::handlers]
pub mod api {
    use super::*;

    // POST /shorten — the body is the long URL; store it under a fresh code.
    pub fn shorten(req: Request, _p: Params) -> Response {
        let target = String::from_utf8_lossy(&req.body).trim().to_string();
        if target.is_empty() {
            return Response::new(400, b"send a URL in the body\n".to_vec());
        }
        let b = kv::bucket("links");
        let code = (b.list().unwrap_or_default().len() + 1).to_string(); // simple sequential code
        let _ = b.set(&code, target.as_bytes());
        Response::new(201, format!("/{code}\n").into_bytes())
    }

    // GET /:code — look the code up and redirect to the URL.
    pub fn expand(_req: Request, p: Params) -> Response {
        let code = p.get("code").unwrap_or("");
        match kv::bucket("links").get(code).ok().flatten() {
            Some(url) => Response::new(302, Vec::new())
                .header("location", String::from_utf8_lossy(&url)),
            None => Response::new(404, b"not found\n".to_vec()),
        }
    }
}
```

```go [Go]
// components/api/main.go — routed actions over durable kv.
package main

import (
	"strconv"
	"strings"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	links := rusm.OpenBucket("links")
	h := web.NewHandlers()

	// POST /shorten — the body is the long URL; store it under a fresh code.
	h.Handle("shorten", func(req web.Request, _ web.Params) web.Response {
		target := strings.TrimSpace(string(req.Body))
		if target == "" {
			return web.Bytes(400, []byte("send a URL in the body\n"))
		}
		keys, _ := links.List()
		code := strconv.Itoa(len(keys) + 1) // a simple sequential code
		_ = links.Set(code, []byte(target))
		return web.Bytes(201, []byte("/"+code+"\n"))
	})

	// GET /:code — look the code up and redirect to the URL.
	h.Handle("expand", func(_ web.Request, p web.Params) web.Response {
		if url, ok, _ := links.Get(p.Get("code")); ok {
			return web.Bytes(302, nil).Header("location", string(url))
		}
		return web.Bytes(404, []byte("not found\n"))
	})

	h.Serve()
}
```

:::

Notice what *isn't* there: no router (the manifest routes), no socket or wire handling (the
platform owns it), no database setup (`kv` is built in). You wrote two functions.

## Run it

```sh
rusm build                                    # compile components/api → ./wasm/
rusm serve                                    # api → http://127.0.0.1:8080

# shorten a URL — the body is the long URL, the reply is the short path:
curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'
# → /1

# follow the code — -i shows the redirect a browser would take:
curl -i 127.0.0.1:8080/1
# → HTTP/1.1 302 Found
# → location: https://rusm.dev/docs
```

That's a working URL shortener. Stop the server and start it again — `curl -i 127.0.0.1:8080/1`
still redirects, because the map lives in the durable `kv` file, not in the process.

## What you just built — the app model

Every RUSM app is this same handful of pieces, and the shortener has exercised them all:

- **A per-request handler.** `api` carries no `resident` flag, so it's **on-demand**: a fresh,
  isolated instance is spawned for each request, runs one action, replies, and is gone. Nothing
  is shared or parked between requests — so a slow or crashing request can't hurt another.
- **State lives outside the handler.** Because the instance is ephemeral, durable state goes in
  **`kv`** (as here) — or, when it's in-memory or computed rather than a simple key→value map,
  in a long-lived **resident service** the handler reaches over the actor API. That's the
  [Build a stateful service](/build-an-app/build-a-stateful-service) pattern; a `resident = true`
  component is boot-spawned and supervised by the node.
- **Default-deny capabilities.** The handler got `allow-storage` and nothing more. Grant only
  what a component needs — [Grant capabilities](/build-an-app/grant-capabilities).
- **The CLI runs it.** `rusm build` compiles `components/*` to `./wasm/`; `rusm serve` binds the
  listeners; `rusm run` and `rusm dev` cover non-serving apps and the watch-reload loop. Start
  from scratch any time with `rusm new <name>`. The full set is the
  [rusm CLI](/build-an-app/the-rusm-cli).

With `[log] level` at `info`+, `rusm serve` also access-logs each request
(`rusm http POST /shorten → 201`, `rusm http GET /1 → 302`) in the same stream as your
`console.log` / `log::info!` / `slog` lines.

## Going further

You've built and run a complete app. From here, the rest of the toolkit slots onto the same
model:

- **Serve more than HTTP** — a live feed or a chat over [SSE](/build-an-app/serve-sse) and
  [WebSocket](/build-an-app/serve-websocket), one process per connection.
- **Split work across components** — [call another component](/build-an-app/call-another-component),
  run [one-off work](/build-an-app/run-one-off-work), or [broadcast to many](/build-an-app/broadcast-to-many).
- **The exact routing rules** (`:param`, `*` wildcard, specificity, 404/405) — [Serve HTTP](/build-an-app/serve-http).
- **Every manifest table and field** — the [configuration reference](/deep-dive/configuration).
