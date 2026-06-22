# A basic app

A RUSM app is a **manifest plus some components**: a `rusm.toml` that declares what your app
is made of, and a `components/` folder holding the source. The `rusm` CLI builds each
component to `./wasm/`, then loads, serves, and supervises them. This is exactly what `rusm
new` scaffolds — this page walks the shape of one. (You can also drive RUSM from your own Rust
binary — an advanced path covered in [Embedding RUSM as a library](/deep-dive/embedding).)

## The shape of an app

```
my-app/
├── rusm.toml          # the manifest — what the app is made of
├── components/
│   ├── api/           # an HTTP handler   (TS index.ts · Rust src/lib.rs · Go main.go)
│   └── counter/       # a resident service
└── wasm/              # rusm build writes the compiled components here
```

One folder per component under `components/`, one `[components.<name>]` entry per component in
the manifest. You write the source; `rusm build` produces `./wasm/<name>.{wasm,js}`.

## A worked example

Here's a small but complete app: a stateless HTTP handler in front of a long-lived service
that holds a counter. It all lives in one `rusm.toml`.

```toml
# rusm.toml

[[serve]]                          # a listener on a real TCP port
protocol = "http"
listen   = "127.0.0.1:8080"

[serve.routes]                     # map each request to a handler action
"GET  /count" = "api#show"
"POST /count" = "api#bump"

[components.api]                   # the HTTP handler — a fresh instance per request
capability = "sandboxed"

[components.counter]               # a resident service — holds the count, supervised
capability = "sandboxed"
resident   = true
```

Two components, two roles — and the difference between them is the heart of the app model.

## Components — `[components.<name>]`

Every `[components.<name>]` entry is **registered** under that name, so a route or a sibling
can reach it. What differs is *when* it runs:

- **On-demand (the default).** Without `resident`, a component is spawned only when something
  asks for it — a per-request HTTP handler (`api` above), or a one-off
  [worker](/build-an-app/run-one-off-work). No idle instance sits parked: a fresh, isolated
  one is spawned per unit of work, and it's gone when it returns.
- **Resident (`resident = true`).** The node **boot-spawns** the component at startup and
  **supervises** it — auto-restarting on crash, bounded by restart-intensity. This is how you
  run a long-lived, stateful [service](/build-an-app/stateful-service) like `counter`:
  something that must stay alive and hold state across requests.

That split *is* the "where does state live" answer: **handlers are ephemeral and per-request;
state lives in a resident service** (reached over the actor API with `whereis` / `call` /
`send`) or in durable [`kv`](/deep-dive/configuration). A handler never keeps state in its own
instance — there's a fresh one every request, so there's nowhere for it to leak.

Each component also carries a **capability** profile (`sandboxed` here — default-deny). That's
its own topic: [Grant capabilities](/build-an-app/capabilities).

## Serving — `[[serve]]` and `[serve.routes]`

A `[[serve]]` entry is a **pure listener**: a protocol and a TCP address, nothing more. It
carries no handler or capability of its own.

- For **HTTP/SSE**, a `[serve.routes]` subtable maps `"METHOD /path"` → `"component#action"`;
  the host resolves each request to the matched handler component (spawned fresh) and action.
- A **WebSocket** listener (or a routes-less HTTP one) names its single per-connection handler
  directly with `component = "..."` instead.

Each listener has its own routes, so multiple ports route independently. The full routing
rules (`:param`, `*` wildcard, specificity, 404/405) are in
[Serve HTTP](/build-an-app/serve-http).

## Build and run

```sh
rusm build        # compile components/* → ./wasm/  (cargo wasm32-wasip2 · TinyGo · Bun)
rusm serve        # bind every [[serve]] listener; boot + supervise resident components
```

Because this app has a `[[serve]]` listener, you run it with **`rusm serve`**. Two more
commands cover the non-serving case and the dev loop:

```sh
rusm run          # spawn the [components.<name>] as supervised processes — for apps with
                  # no listeners (pure workers, services, CLIs)
rusm dev          # build + run, then watch ./components and reload the changed one on edit
```

`rusm new <name>` scaffolds all of this ready to serve — a component, a `rusm.toml` with a
`[[serve]]` entry, `.gitignore`, and a README — so `rusm new hello && cd hello && rusm build
&& rusm serve` gives you a live server. The full command set is the
[rusm CLI](/build-an-app/cli).

With `[log] level` at `info`+, `rusm serve` also access-logs each served request
(`rusm http GET /count → 200`, an SSE stream as `sse`, a WS upgrade as `ws … → 101`), in the
same stream as lifecycle and guest logs.

## Beyond the basics

The manifest has more to offer when you need it — none of it required to get started:

- **Custom capability profiles** — define your own grants beyond the three built-ins
  (`sandboxed` / `network-client` / `trusted`), like Cargo's `[profile.<name>]`. See
  [Grant capabilities](/build-an-app/capabilities).
- **Your own native functions** — call host code (a database client, a signing routine) from
  any guest via a [bridge](/build-an-app/custom-bridges).
- **A `[node]` table** — set the node's attach port and scheduler profile so you can
  [observe a running node](/deep-dive/observe) live. Optional; the defaults are fine.
- **Every table and field** — the exhaustive [configuration reference](/deep-dive/configuration).
