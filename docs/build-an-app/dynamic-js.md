# Dynamic JS

A RUSM TypeScript guest runs on a shared, predefined **js-runner** (a WASM component that
evaluates a JS bundle). Because the *code* is just a bundle the runner loads, you don't have
to bake it in at build time — you can supply it **at deploy time** or **at runtime**. That
unlocks two things:

- **Deploy JS live** — point a component at a URL or the durable KV store; replace the bundle
  there and the next instance runs the new code, **no node rebuild**.
- **Run code chosen at runtime** — hand a runner template a bundle your app *generates* or
  *fetches on the fly*, and it runs inside a sandbox **you** define. The operator fixes the
  capabilities; the guest picks the code, never the permissions. Perfect for user-submitted
  plugins, AI-generated code, or per-tenant logic.

## Deploy live — `source`

Give any `[components.<name>]` or `[[serve]]` entry a **`source`** and it loads its JS bundle
from there instead of the local `./wasm/<name>` artifact:

```toml
[node]
store = "data/app.redb"                  # required for kv: sources

# A routes-less HTTP handler whose code lives at a URL — redeploy by replacing the bundle:
[[serve]]
component = "api"
protocol  = "http"
listen    = "127.0.0.1:8080"
source    = "https://cdn.example/api.js"

# A component whose bundle is published to the durable KV store:
[components.worker]
source = "kv:bundles/worker"
```

| `source` | Resolves to |
| --- | --- |
| `https://…` (or `url:<u>`) | an HTTP(S) GET — a presigned blob, an artifact API; a non-2xx fails loudly |
| `kv:<bucket>/<key>` | an entry in the node's durable `store` (needs `[node] store`) |
| _(omitted)_ | the local `./wasm/<name>` artifact — the default |

A `[components.<name>]` process fetches its bundle **once** at spawn (and again on each `rusm
dev` reload); a `[[serve]]` listener fetches **once at bind**, then every ephemeral serving
instance runs from that bundle. To ship new code, update the source and re-spawn / restart the
listener — the node binary never changes.

## Run code chosen at runtime — `dynamic = "js"`

When the bundle is only known *at runtime* — generated in-process, or a key/URL a guest
computes — declare a **runner template**: a capability profile with **no fixed bundle**. A
guest can't `spawn` it directly; it spawns instances with a runtime source, and the loaded JS
runs under the template's **declared** profile:

```toml
# The box. The guest picks the code; the operator fixes the capabilities, always.
[components.sandbox-runner]
capability = "sandboxed"     # e.g. no network, no storage — whatever you grant here
dynamic    = "js"
```

A guest then runs code in that box — the source is **`inline:<js>`** (a string, e.g. code it
just generated), **`kv:<bucket>/<key>`**, or **`url:`/`http(s)://…`**:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// `Process.spawn(template, source)` — the second arg makes it a dynamic-JS spawn.
Process.spawn("sandbox-runner", `inline:${generatedJs}`);        // code generated in-process
Process.spawn("sandbox-runner", "kv:jobs/" + jobId);             // published to the KV store
Process.spawn("sandbox-runner", "url:https://cdn.example/job.js"); // fetched by the node
```

```rust [Rust]
// run code chosen at runtime under the `sandbox-runner` profile:
rusm_rs::spawn_from("sandbox-runner", &format!("inline:{generated_js}"))?;
rusm_rs::spawn_from("sandbox-runner", &format!("kv:jobs/{job_id}"))?;
rusm_rs::spawn_from("sandbox-runner", "url:https://cdn.example/job.js")?;
```

```go [Go]
rusm.SpawnFrom("sandbox-runner", "inline:"+generatedJS)
rusm.SpawnFrom("sandbox-runner", "kv:jobs/"+jobID)
rusm.SpawnFrom("sandbox-runner", "url:https://cdn.example/job.js")
```

:::

The loaded JS **always runs under the template's profile** — so untrusted or generated code is
boxed by the operator, never by the caller. That's the safety guarantee: a guest chooses *what*
runs; it can never widen *what it's allowed to do*.

## What you need to know

- **Capability-gated.** Spawning needs the `spawn` capability; a `kv:` source additionally
  needs `storage`, and a `url:` source needs `network`. An `inline:` bundle needs neither (no
  I/O). The fetch for `url:` is a **host** action — the node owns egress, so the sandbox itself
  never gets network unless you grant it.
- **The runner is shared.** Every dynamic-JS instance rides the same ~920 KB js-runner — you
  ship the engine once, not per bundle (see [guests](/deep-dive/guests)).
- **JS or compiled WASM.** This page is the **JS** kind (`dynamic = "js"`). Its twin,
  [dynamic WASM](/build-an-app/dynamic-wasm) (`dynamic = "wasm"`), loads a **compiled WASM
  component** chosen at runtime — compiled once, then cached for hot re-spawns. The `"js"`
  string also anticipates other **interpreted** runners (e.g. Python, Ruby). A compiled
  `.wasm` can still ship the ordinary way too, as a build-time artifact (`rusm build` →
  `./wasm/`) — see [the serving model](/deep-dive/the-serving-model).
- **Full field reference:** every `source` / `dynamic` rule is in the
  [configuration reference](/deep-dive/configuration#dynamic-bundle-sourcing).
