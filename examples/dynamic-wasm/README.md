# Dynamic WASM — runtime-chosen compiled plugins

Run **compiled WASM components chosen at runtime**, each inside a sandbox the *operator*
fixes. The node declares a `plugin-runner` capability profile with **no fixed bundle**; an
HTTP request picks which `.wasm` plugin to load (`kv:plugins/<name>`). The plugin is
**compiled once on its first use** (cold) and served from a content-addressed cache on every
later spawn (hot) — measured here at **~17 ms cold → ~0.5 ms hot** (~30×). The request
chooses *which* code runs; it can never widen *what that code may do*.

This is the runnable companion to the docs guide
[*Dynamic WASM*](../../docs/build-an-app/dynamic-wasm.md).

## What's here

| Piece | Role |
| --- | --- |
| `components/api` | the **dispatcher** — `GET /run/:plugin/:input` spawns the chosen plugin, hands it the input, returns the answer |
| `components/greeter` | a plugin (`Hello, <input>!`) — a normal compiled actor component, **not** declared in `rusm.toml` |
| `components/shout` | a second plugin (`<INPUT>`) — same runner, different `.wasm` |
| `rusm.toml` | the `plugin-runner` template (`dynamic = "wasm"`) + the dispatcher's caps |

The plugins are ordinary components; `rusm build` compiles them to `wasm/`. They're never
*hosted* — you publish them to the node's durable store and the dispatcher spawns them by name.

## Run it

The store is single-writer, so publish the plugins while the node is **stopped**, then serve:

```sh
cd examples/dynamic-wasm
rusm build                                   # compiles api, greeter, shout → wasm/

rusm kv set plugins/greeter wasm/greeter.wasm   # publish the plugin bundles
rusm kv set plugins/shout   wasm/shout.wasm
rusm kv list plugins                            # greeter, shout

rusm serve                                   # http://127.0.0.1:8080
```

Then, in another shell:

```sh
curl 127.0.0.1:8080/run/greeter/World    # → Hello, World!   (compiled cold on this first hit)
curl 127.0.0.1:8080/run/greeter/RUSM     # → Hello, RUSM!    (hot — cached compile)
curl 127.0.0.1:8080/run/shout/hello      # → HELLO           (a different plugin, same runner)
curl 127.0.0.1:8080/run/missing/x        # → 404 no plugin `missing`
```

## Adding or updating a plugin

Add a `components/<name>/` (any language that compiles to a `wasm32-wasip2` actor
component), then `rusm build` and publish it — no change to `rusm.toml`, no new code in the
dispatcher:

```sh
rusm kv set plugins/<name> wasm/<name>.wasm
```

Because `rusm kv` writes the store directly, **stop the node first** (it holds the store
lock). To redeploy a plugin **without restarting**, point the runner at a `url:` source
instead of `kv:` — the node re-fetches from the URL once the compile cache's freshness window
(`[node] dynamic_wasm_ttl_secs`, default 300 s) elapses, so replacing the artifact at that URL
rolls the plugin out live. See the docs guide for the `url:` flow.

## How the safety works

The dispatcher's profile grants `allow-spawn` (it may spawn the runner) and `allow-storage`
(the `kv:` fetch is gated on it) — nothing else. The plugins run under the **`plugin-runner`**
profile (`sandboxed` here: no network, no storage), no matter who spawns them or what bundle
is chosen. So an operator can host untrusted, generated, or per-tenant compiled code and know
exactly what it can touch — the box is declared in one place, in `rusm.toml`.
