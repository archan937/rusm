# The app model

This is the path most apps take — and what `rusm new` scaffolds: declare your components in
a `rusm.toml` manifest and let the `rusm` CLI build, load, serve, and supervise them from
`./wasm/`. (You can also drive RUSM from your own Rust binary — an advanced path covered in
[Embedding RUSM as a library](./embedding).)

```toml
# rusm.toml
[node]
listen = "127.0.0.1:4000"
profile = "balanced"

[components.worker]      # loaded from ./wasm/worker.wasm
capability = "sandboxed" # a built-in or a custom profile (below)
resident = true          # long-lived service: boot-spawned + supervised
```

```sh
rusm run          # load every [components.<name>] from ./wasm/, register them, boot
                  # + supervise the resident ones
```

A component keyed `[components.<name>]` is always **registered** so a route or a sibling
can `spawn` it by name. `resident = true` additionally makes the node **boot-spawn** it
at startup and **supervise** it (auto-restart on crash, bounded by restart-intensity).
Without `resident`, it is spawned only on demand (a per-request handler, an on-demand
worker) — no idle parked instance.

**Serving on a real port.** To run a component as an HTTP / WS / SSE server, declare
a `[[serve]]` listener and run `rusm serve` — it binds each on its TCP `listen` address.
A `[[serve]]` entry is a **pure listener**: a routed HTTP/SSE listener names its handlers
in `[serve.routes]` (each a `[components.<name>]` entry that carries its own capability);
a WS or routes-less HTTP listener names its single handler with `component`. The fastest
way in is **`rusm new <name>`**, which scaffolds a ready-to-serve app (a zero-dependency
TS HTTP component, a `rusm.toml` with a `[[serve]]` entry, `.gitignore`, README):

```toml
[[serve]]
component = "api"         # the per-connection handler → ./wasm/api.{wasm,js}
protocol = "ws"           # "http" | "sse" | "ws"
listen = "127.0.0.1:8080"
```

```sh
rusm new hello && cd hello
rusm build
rusm serve
curl http://127.0.0.1:8080/
```

With `[log] level` at `info`+, `rusm serve` access-logs each served request —
`rusm http GET / → 200`, an SSE stream as `sse`, a WS upgrade as `ws … → 101` — in the
same stream as the lifecycle and guest logs.

**Custom capability profiles.** Beyond the three built-ins (`sandboxed` /
`network-client` / `trusted`), you can define your own — like Cargo's
`[profile.<name>]`. A profile `inherits` a built-in base (default `sandboxed`,
default-deny) and overrides only the grants it sets; a component references it by
name — and a node-registered component runs under **its own** declared profile,
whoever spawns it (the `allow-spawn` capability gates who may spawn; a guest can't fabricate
grants the operator never declared).

```toml
[capabilities.agent]
inherits = "network-client"   # base; omit for sandboxed (default-deny)
allow-spawn = true            # may spawn other components by name
max-memory-mb = 256
env = ["OPENAI_API_KEY"]      # grant these keys (values from process env / .env)
preopen = [{ host = "./data", guest = "/data", read-only = false }]

[components.pages-agent]
capability = "agent"          # resolves to the custom profile above
```
