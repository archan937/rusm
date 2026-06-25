# Grant capabilities

Every component starts with **nothing** — default-deny. It gets CPU and a bounded heap, and
that's all: no network, no spawning, no storage, no filesystem, no environment variables. You
grant *exactly* what a component needs and nothing more, and a breach — a denied call or
blowing the memory cap — traps **only that process**, never the node.

## The three built-in profiles

A component picks a profile by name:

| Profile | Grants |
| --- | --- |
| `sandboxed` *(default)* | CPU + a bounded heap. Nothing else. |
| `network-client` | + outbound network (`fetch` / `wasi:http`). |
| `trusted` | a broad grant — for components you fully control. |

## Grant in `rusm.toml`

For anything beyond a preset, define a custom profile — like Cargo's `[profile.<name>]`:
`inherits` a base, then override **only** the grants you add. A component references it by
name:

```toml
[capabilities.agent]
inherits = "network-client"     # base (omit for sandboxed, the default-deny base)
allow-spawn = true              # may spawn other components by name
allow-process-control = true    # may monitor / kill / inspect other processes
allow-storage = true            # may use the durable kv store
max-memory-mb = 256             # heap ceiling
env = ["OPENAI_API_KEY"]        # the only env keys it can read (values from process env / .env)
preopen = [{ host = "./data", guest = "/data", read-only = false }]
bridges = ["weather"]           # may call your own `weather` function (a custom bridge)

[components.assistant]
capability = "agent"            # this component runs under that profile
```

Each line gates a specific power — outbound network, spawning, process control, storage, which
env vars it sees, which host dirs are mounted, which of [your own functions](/build-an-app/add-your-own-functions)
it may call, and its memory ceiling. Everything not granted is denied.

The crucial property: a node-registered component always runs under **its own** declared
profile — *whoever* spawns it. So a secret you grant a component stays scoped to it, and a
guest can never fabricate a grant the operator didn't declare.

## Common profiles

A handful of recipes cover most components — match the grant to what the component actually
does:

```toml
# A web client — calls an API or an LLM over HTTPS.
[capabilities.web]
inherits = "network-client"

# A stateful service — reads/writes the durable kv store.
[capabilities.stateful]
inherits = "sandboxed"
allow-storage = true

# An orchestrator — spawns and supervises other components.
[capabilities.orchestrator]
inherits = "sandboxed"
allow-spawn = true
allow-process-control = true

# An SSE / WebSocket handler — monitors its writer process to detect disconnect.
[capabilities.streamer]
inherits = "sandboxed"
allow-process-control = true
```

These map straight onto the common patterns: a [stateful service](/build-an-app/build-a-stateful-service)
wants `allow-storage`; [calling another component](/build-an-app/call-another-component) wants
`allow-spawn`; a [broadcast](/build-an-app/broadcast-to-many) / SSE / WS handler wants
`allow-process-control`. Grant only the line each one needs.

## When embedding

Driving RUSM as a [library](/deep-dive/embedding-rusm-as-a-library)? Grant the same powers with the
`Capabilities` builder, per spawn:

```rust
use rusm_wasm::{Capabilities, CapabilityProfile};

CapabilityProfile::Sandboxed.capabilities();          // CPU + bounded heap only
Capabilities::nothing()                               // …or start from nothing
    .max_memory(16 << 20)                             // a 16 MiB ceiling
    .allow_network(true)                              // outbound sockets
    .preopen("/srv/data", "/data", /* read_only */ true)
    .env("LOG", "info");
```

Grants map onto standard WASI plus a `StoreLimiter` memory cap. Every field is in the
[configuration reference](/deep-dive/configuration); the model in depth is
[permissions & sandboxing](/deep-dive/permissions-and-sandboxing).
