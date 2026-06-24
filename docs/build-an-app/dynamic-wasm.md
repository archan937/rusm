# Dynamic WASM

A RUSM component is normally a **compiled** `.wasm` you build ahead of time (`rusm build` →
`./wasm/`). But the bytes are just an artifact the runtime loads — so you can also supply a
compiled component **at deploy time** or **choose one at runtime**, and run it inside a
sandbox *you* define. RUSM makes the compile a one-time cost.

- **Compile once, spawn hot forever** — the first spawn of a bundle compiles it (cold);
  every later spawn instantiates the cached, prepared component on the pooled fast path
  (hot). Measured on a small component: **~17 ms cold → ~0.5 ms hot** (~30×).
- **Run code chosen at runtime** — hand a runner template a compiled component your app
  selects, fetches, or generates, and it runs in a sandbox **you** fixed. The operator fixes
  the capabilities; the request picks the code, never the permissions. Untrusted plugins,
  per-tenant logic, an artifact registry of components.

Three runner flavours — pick by the component's WIT world:

| | `dynamic = "wasm"` | `dynamic = "wasi-cli"` | `dynamic = "js"` |
|---|---|---|---|
| **WIT world** | `rusm:runtime` (RUSM actor) | `wasi:cli/run` (any wasm32-wasip2) | rquickjs JS bundle |
| **Entry export** | `"run"` (or `entry =`) | fixed (`run` in the cli world) | — |
| **`spawn-from` returns** | pid (actor — send/receive) | pid (runs to completion) | pid (TS actor) |
| **Compile cache** | ✓ by content hash + entry | ✓ by content hash | — (js-runner is fixed) |

The runnable companion is [`examples/dynamic-wasm`](https://github.com/archan937/rusm/tree/main/examples/dynamic-wasm).

## RUSM actor — `dynamic = "wasm"`

Declare a **runner template**: a capability profile with **no fixed bundle**. A guest can't
`spawn` it; it runs only via `spawn-from(name, source)`, and the loaded component runs under
the template's **declared** profile:

```toml
[node]
store = "plugins.redb"           # the durable store the bundles live in (for kv: sources)

# The box. The request picks the compiled component; the operator fixes the capabilities.
[components.plugin-runner]
capability = "sandboxed"         # e.g. no network, no storage — whatever you grant here
dynamic    = "wasm"
# entry   = "handle"             # override the entry export name (default "run")
```

A guest then runs a compiled component in that box. The source is **`kv:<bucket>/<key>`** (a
bundle in the node store), **`url:`/`http(s)://…`** (fetched by the node), or **`inline:`**
(raw bytes — rare for WASM):

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

Process.spawn("plugin-runner", `kv:plugins/${name}`);
Process.spawn("plugin-runner", "url:https://cdn.example/plugin.wasm");
```

```rust [Rust]
// run a compiled component chosen at runtime, under the `plugin-runner` profile:
let pid = rusm_rs::spawn_from("plugin-runner", &format!("kv:plugins/{name}"))?;
let pid = rusm_rs::spawn_from("plugin-runner", "url:https://cdn.example/plugin.wasm")?;
```

```go [Go]
rusm.SpawnFrom("plugin-runner", "kv:plugins/"+name)
rusm.SpawnFrom("plugin-runner", "url:https://cdn.example/plugin.wasm")
```

:::

The spawned component is an ordinary actor component — it `receive`s messages and `send`s
replies, exactly like a built-in one. The dispatcher talks to it over the actor wire:

```rust
// A per-request HTTP handler that runs a runtime-chosen plugin and returns its answer.
let pid = match spawn_from("plugin-runner", &format!("kv:plugins/{plugin}")) {
    Ok(pid) => pid,
    Err(e) => return Response::new(404, format!("no plugin `{plugin}`: {e}\n").into_bytes()),
};
send_bytes(pid, format!("{}\n{input}", me().0).as_bytes()); // "<reply-to>\n<input>"
match receive_bytes_timeout(5_000) {
    Some(reply) => Response::new(200, reply),
    None => Response::new(504, b"plugin timed out\n".to_vec()),
}
```

## The compile cache — cold once, hot forever

The expensive step is compiling a fetched bundle. RUSM does it **at most once per distinct
bundle**, keyed by the **content hash** (the SHA-256 of the bytes), not the source string:

- **First spawn (cold)** — fetch, hash, compile, prepare, then spawn. The prepared component
  is cached under its content hash.
- **Every later spawn (hot)** — the source is still fresh, so its hash is looked up and the
  prepared component spawned directly: **no fetch, no compile**, on the same pooled fast path
  as a built-in component.

Because the key is the *bytes*: two sources serving identical bytes compile **once**; a
source that starts serving new bytes is a **new** key (a new compile), so a redeploy is
picked up without ever serving stale code. Concurrent first-spawns of the same bundle are
**single-flighted** — they compile once and the rest await, never a thundering herd. A
compile that fails is **not** cached (the error surfaces; the next spawn retries).

### Freshness — `dynamic_wasm_ttl_secs`

A source is considered fresh for a TTL window (`[node] dynamic_wasm_ttl_secs`, default
**300 s**). Within it, spawns are fully hot (no re-fetch). After it, the next spawn re-checks
the source for new bytes — same bytes reuse the existing compile; changed bytes recompile
under the new hash. The same window also evicts a compiled artifact left **unused** for the
TTL, so memory tracks what's actually in play.

```toml
[node]
dynamic_wasm_ttl_secs = 60     # re-check sources every minute (faster live-redeploy pickup)
```

## Publish a bundle — `rusm kv`

A `kv:` source needs the bytes **in** the store first. Publish them with the
[`rusm kv`](/build-an-app/the-rusm-cli#rusm-kv) command (the node must be
stopped — the store is single-writer):

```sh
rusm build                                      # compile components/* → wasm/
rusm kv set plugins/greeter wasm/greeter.wasm   # publish a compiled plugin
rusm kv list plugins                            # greeter
rusm serve
```

For a redeploy **without restarting the node**, use a **`url:`** source instead: the node
re-fetches from the URL once the TTL elapses, so replacing the artifact behind that URL (a
blob store, an artifact API, a CDN) rolls the new component out live. `kv:` writes need the
node stopped; `url:` sourcing is the live-redeploy path.

## Stock CLI tools — `dynamic = "wasi-cli"`

Sometimes you want to run an **existing** binary — a CLI tool, a batch script, a utility —
that knows nothing about RUSM's actor world. Any `wasm32-wasip2` component that implements
the standard `wasi:cli/run` world works here; no `rusm:runtime` imports, no actor wire, no
`spawn-from` protocol to implement.

```toml
[node]
store = "tools.redb"

[components.image-processor]
capability  = "sandboxed"
dynamic     = "wasi-cli"
```

The guest spawns it exactly like any other template — `spawn-from(name, source)` fetches
the `.wasm`, compiles it (once, cached by content hash), builds the `wasi:cli/run`
instantiation, and runs it as a one-shot sandboxed process. The process runs to completion
and exits normally; `spawn-from` returns a pid you can monitor or ignore:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Run an image-processing binary stored in the node's kv store.
const pid = Process.spawnFrom("image-processor", "kv:tools/img-proc.wasm");
// Optionally monitor it — `__down` fires when it exits.
const ref = Process.monitor(pid);
```

```rust [Rust]
// Fire-and-forget: spawn a CLI tool, monitor so we know when it finishes.
let pid = rusm_rs::spawn_from("image-processor", "kv:tools/img-proc.wasm")?;
let mon = rusm_rs::monitor(pid);
```

```go [Go]
pid, _ := rusm.SpawnFrom("image-processor", "kv:tools/img-proc.wasm")
mon := rusm.Monitor(pid)
```

:::

`entry =` is ignored for `wasi-cli` templates — the `wasi:cli/run` entry protocol is fixed.

## Custom entry export — `entry`

Every RUSM actor component exports `run: func()` by default. If your component names its
entry export differently — say, a component built against a custom WIT world that exports
`start: func()` — use `entry =`:

```toml
[components.my-worker]
entry = "start"           # override the default "run"; applies to static + dynamic = "wasm"

[components.dynamic-runner]
dynamic = "wasm"
entry   = "handle"        # dynamic WASM also obeys entry; keyed separately in the cache
```

The entry name is part of the compile-cache key for `dynamic = "wasm"` templates: the same
`.wasm` bytes compiled for entry `"run"` and for `"handle"` are prepared and cached
**independently**. This is uncommon — most components export `run` — but important when
building against a non-standard WIT world.

> `entry =` has no effect on `dynamic = "js"` (the JS runner has its own protocol) or
> `dynamic = "wasi-cli"` (the `wasi:cli/run` entry is fixed by the standard).

## Deploy a remote component — `source`

You don't need a template to load a remote `.wasm`. Give any `[components.<name>]` or
`[[serve]]` a **`source`** and it loads a compiled component (or a JS bundle — RUSM tells
them apart by the WASM magic number) from there instead of the local `./wasm/<name>`:

```toml
# A handler component whose compiled bundle is published to the durable store:
[components.api]
source = "kv:bundles/api"

# …or pulled from an artifact URL — redeploy by replacing the bundle there:
[[serve]]
component = "api"
protocol  = "http"
listen    = "127.0.0.1:8080"
source    = "https://cdn.example/api.wasm"
```

## What you need to know

- **Capability-gated.** Spawning needs the `spawn` capability; a `kv:` source additionally
  needs `storage`, and a `url:` source needs `network`. The gate is enforced on **every**
  spawn — cold *and* hot — so a cached bundle can never be reached by a guest lacking the
  capability. An `inline:` bundle needs neither (no I/O). The `url:` fetch is a **host**
  action (the node owns egress), so the sandbox itself never gets network unless you grant it.
- **The chosen code runs under the template's profile, always.** A guest picks *which*
  compiled component runs; it can never widen *what it's allowed to do*. That's the safety
  guarantee — host untrusted, generated, or per-tenant compiled code with confidence.
- **Any source language.** Both `"wasm"` and `"wasi-cli"` compile to `wasm32-wasip2`; the
  difference is the **WIT world** they implement. `"wasm"` plugins import `rusm:runtime` (the
  actor ABI — spawn, send, receive). `"wasi-cli"` tools implement only `wasi:cli/run` — no
  RUSM SDK, no actor wire, any standard CLI toolchain works.
- **Full field reference:** every `source` / `dynamic` / `entry` / `dynamic_wasm_ttl_secs`
  rule is in the [configuration reference](/deep-dive/configuration#dynamic-bundle-sourcing).
