# Multi-tenant bridges

A [custom bridge](/build-an-app/add-your-own-functions) is one shared host implementation,
but it doesn't have to treat every caller the same. A single `gql()` can talk to client X's
GraphQL endpoint with X's credentials and client Y's with Y's — the question is only *where
the per-tenant secret lives* and *how the bridge knows who's calling*. There are two honest
answers, with different trust models; pick by whether your tenants must be isolated from one
another.

## The real boundary is the capability profile

A component reaches a bridge only when its [capability profile](/build-an-app/grant-capabilities)
grants it (`bridges = ["gql"]`), and the bridge gates itself default-deny on
`self.caps().allows_bridge("gql")`. The profile is **operator-controlled and a guest cannot
forge it** — so it, not the caller's self-asserted name, is the boundary to lean on for
anything sensitive.

For credential **isolation** between tenants, scope each tenant's secret to its own profile.
Grant component X only X's environment keys and component Y only Y's:

```toml
[capabilities.client-x]
inherits = "network-client"
bridges  = ["gql"]
env      = ["X_GQL_URL", "X_GQL_TOKEN"]   # only these reach X's sandbox

[capabilities.client-y]
inherits = "network-client"
bridges  = ["gql"]
env      = ["Y_GQL_URL", "Y_GQL_TOKEN"]   # only these reach Y's sandbox

[components.client-x]
capability = "client-x"

[components.client-y]
capability = "client-y"
```

Each guest reads its **own** granted env (it cannot see the other tenant's keys — they were
never placed in its sandbox) and passes the endpoint + token to the bridge call. The bridge
stays stateless about credentials. This is the answer when X and Y must not be able to reach
each other's secrets: the isolation is enforced by the platform, not by trusting a name.

## Resolving per-caller config inside the bridge (Rust host)

When the components are **your own** and you'd rather keep the secrets host-side — so they
never enter any guest — a **Rust** host bridge can resolve them per caller. Every bridge
method is `async fn(&mut self, …)` on `BridgeHost`, which hands you the caller on each call:

- `self.pid()` — the calling process's pid (host-assigned).
- `self.runtime()` — the actor runtime, to look the caller up in the registry.
- `self.caps()` — the caller's capability grants (`allows_bridge`, `storage_allowed`, …).

The component identifies itself by the name it registers; the bridge maps that name to
config it loaded host-side:

```rust
use std::collections::HashMap;
use std::sync::OnceLock;

use rusm_otp::Pid;                          // add `rusm-otp` to the bridge crate to name Pid
use rusm_wasm::wasmtime::component::HasSelf;
use rusm_wasm::{wasmtime, BridgeHost, BridgeLinker};

use crate::bindings::app::gql::gql;         // generated from bridges/gql/bridge.wit

struct Tenant {
    endpoint: String,
    token: String,
}

/// Loaded once, host-side — no guest ever sees it.
fn tenants() -> &'static HashMap<String, Tenant> {
    static T: OnceLock<HashMap<String, Tenant>> = OnceLock::new();
    T.get_or_init(|| {
        // …load from env / a config file / the kv store…
        HashMap::new()
    })
}

pub fn add_to_linker(linker: &mut BridgeLinker) -> wasmtime::Result<()> {
    gql::add_to_linker::<_, HasSelf<BridgeHost>>(linker, |host| host)
}

impl gql::Host for BridgeHost {
    async fn query(&mut self, q: String) -> Result<String, String> {
        // The caller identifies itself by the name it registered (e.g. Process.register).
        let pid = Pid::from_raw(self.pid());
        let tenant = self
            .runtime()
            .info(pid)
            .and_then(|info| info.names.into_iter().next())
            .ok_or("caller registered no tenant name")?;
        let creds = tenants()
            .get(&tenant)
            .ok_or_else(|| format!("no GraphQL credentials for {tenant}"))?;
        // …run the GraphQL request against creds.endpoint with creds.token…
        Ok(run_query(&creds.endpoint, &creds.token, &q))
    }
}
```

The guest registers its tenant name once, then just calls the bridge — the credentials never
enter it:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";
Process.register("client-x");
// later: gql.query("{ orders { id } }")
```

```rust [Rust]
rusm_rs::register("client-x");
// later: gql::query("{ orders { id } }")
```

```go [Go]
rusm.Register("client-x")
// later: gql.Query("{ orders { id } }")
```

:::

::: warning The registered name is guest-asserted
`runtime().info(pid)` returns the name the **component itself** registered — convenient for
your own cooperating components, but it is **not** a sandbox boundary: a component could
register a different name and select another tenant's credentials. If your tenants are
mutually distrusting, isolate their secrets with capability profiles (the section above),
which the operator controls and a guest can't forge.
:::

## TypeScript and Go host bridges

A `host.ts` / `host.go` bridge runs as a resident actor reached over the actor wire from a
generated Rust shim, and its exported function receives **only the call's arguments** — not
the caller's identity. So bridge-side per-caller resolution is a Rust-host capability today.
A TS or Go host serves multiple tenants by either:

- the caller passing a tenant id as an argument (`gql.query(tenant, "{ … }")`), or
- the capability-profile scoping above, where each tenant's secret lives in its own
  component and the call carries it.

## Where the config lives

Host-side: process env, a config file, or the durable `kv` store — loaded once (a
`OnceLock`) and held in the bridge module, never granted to a guest. That's the point of
keeping resolution in the bridge: the secret stays on the host side of the sandbox.
