# A URL shortener

The best way to learn the shape of a RUSM app is to build one. We'll build a tiny **URL
shortener** — `POST` a long URL, get back a short code; visit the code, get redirected — and
use it to meet the anatomy *every* RUSM app shares: a `rusm.toml` manifest plus some
components, which the `rusm` CLI builds, serves, and supervises.

It's a good first app because it has exactly **two parts**, and they are the two roles
everything in RUSM is built from:

- a **handler** that answers each HTTP request and then disappears, and
- a **service** that stays alive and remembers the `code → URL` map between requests.

Keep that pair in mind — the whole app model is just how you declare it. (You can also drive
RUSM from your own Rust binary instead of the CLI — an advanced path covered in
[Embedding RUSM as a library](/deep-dive/embedding-rusm-as-a-library).)

## The shape of an app

```
shortener/
├── rusm.toml          # the manifest — what the app is made of
├── components/
│   ├── api/           # the HTTP handler   (TS index.ts · Rust src/lib.rs · Go main.go)
│   └── links/         # the resident store (code → URL)
└── wasm/              # rusm build writes the compiled components here
```

One folder per component under `components/`, one `[components.<name>]` entry per component in
the manifest. You write the source; `rusm build` produces `./wasm/<name>.{wasm,js}`.

## The manifest

Everything that makes our shortener an app lives in one `rusm.toml`:

```toml
# rusm.toml

[[serve]]                          # a listener on a real TCP port
protocol = "http"
listen   = "127.0.0.1:8080"

[serve.routes]                     # map each request to a handler action
"POST /shorten" = "api#shorten"    # take a long URL, return a short code
"GET  /:code"   = "api#expand"     # look the code up, redirect to the URL

[components.api]                    # the HTTP handler — a fresh instance per request
capability = "sandboxed"

[components.links]                  # a resident service — holds code → URL, supervised
capability = "sandboxed"
resident   = true
```

Three tables, and they're the whole model: a **listener** (`[[serve]]`) with its **routes**,
and the two **components** the routes lean on. Let's read them in the order they matter.

## Two roles: handler and service

Every `[components.<name>]` entry is **registered** under that name, so a route or a sibling
can reach it. What differs between our two components is *when* each one runs — and that is
the single most important idea in the app model:

- **`api` is on-demand (the default).** Without `resident`, a component is spawned only when
  something asks for it. Each HTTP request gets a **fresh, isolated `api` instance**; it runs
  the matched action, replies, and is gone. Nothing is parked between requests. (A one-off
  [worker](/build-an-app/run-one-off-work) is the same idea without HTTP.)
- **`links` is resident (`resident = true`).** The node **boot-spawns** it at startup and
  **supervises** it — auto-restarting on crash, bounded by restart-intensity. That's how a
  long-lived, stateful [service](/build-an-app/build-a-stateful-service) stays alive to remember the
  `code → URL` map across every request.

So **where does the shortener keep its data?** Not in `api` — there's a new `api` every
request, with nowhere to keep anything. It keeps it in `links`, the resident service, which
`api` reaches over the actor API (`whereis` / `call` / `send`). State that must survive a node
restart goes one step further, into durable [`kv`](/deep-dive/configuration). **Ephemeral
handler in front, durable service (or `kv`) behind** — that's the pattern you'll reach for
again and again.

Each component also carries a **capability** profile (`sandboxed` here — default-deny, the
safe starting point). Granting more is its own topic: [Grant capabilities](/build-an-app/grant-capabilities).

## Serving the routes

A `[[serve]]` entry is a **pure listener** — a protocol and a TCP address, nothing more. It
carries no handler or capability of its own; the `[serve.routes]` subtable does the wiring:

- For **HTTP/SSE**, routes map `"METHOD /path"` → `"component#action"`; the host resolves each
  request to the matched handler component (spawned fresh) and runs that action. Our two
  routes both point at `api`, into its `shorten` and `expand` actions.
- A **WebSocket** listener (or a routes-less HTTP one) instead names its single per-connection
  handler directly with `component = "..."`.

Each listener has its own routes, so multiple ports route independently. The full routing
rules (`:param`, the `*` wildcard, specificity, 404/405) are in
[Serve HTTP](/build-an-app/serve-http).

## Build and run

Because the app has a `[[serve]]` listener, you build it and bring it up with **`rusm serve`**:

```sh
rusm build        # compile components/* → ./wasm/  (cargo wasm32-wasip2 · TinyGo · Bun)
rusm serve        # bind every [[serve]] listener; boot + supervise resident components
```

Two more commands cover the non-serving case and the dev loop:

```sh
rusm run          # spawn the [components.<name>] as supervised processes — for apps with
                  # no listeners (pure workers, services, CLIs)
rusm dev          # build + run, then watch ./components and reload the changed one on edit
```

Don't have a project yet? **`rusm new <name>`** scaffolds a ready-to-serve app — a component,
a `rusm.toml` with a `[[serve]]` entry, `.gitignore`, and a README — so `rusm new hello && cd
hello && rusm build && rusm serve` gives you a live server in four commands. The full command
set is the [rusm CLI](/build-an-app/the-rusm-cli).

With `[log] level` at `info`+, `rusm serve` also access-logs each served request
(`rusm http POST /shorten → 200`, an SSE stream as `sse`, a WS upgrade as `ws … → 101`), in
the same stream as lifecycle and guest logs.

## Now build the parts

You've seen the whole app on paper; the two components are short to write:

- **The `api` handler** — write the `shorten` / `expand` actions: [Serve HTTP](/build-an-app/serve-http).
- **The `links` service** — write the resident store it talks to: [Build a stateful service](/build-an-app/build-a-stateful-service),
  and reach it from `api` with [Call another component](/build-an-app/call-another-component).

## Beyond the basics

The manifest has more to offer when you need it — none of it required to get started:

- **Custom capability profiles** — define your own grants beyond the three built-ins
  (`sandboxed` / `network-client` / `trusted`), like Cargo's `[profile.<name>]`. See
  [Grant capabilities](/build-an-app/grant-capabilities).
- **Your own native functions** — call host code (a database client, a signing routine) from
  any guest via a [bridge](/build-an-app/add-your-own-functions).
- **A `[node]` table** — set the node's attach port and scheduler profile so you can
  [observe a running node](/deep-dive/observe-a-running-node) live. Optional; the defaults are fine.
- **Every table and field** — the exhaustive [configuration reference](/deep-dive/configuration).
