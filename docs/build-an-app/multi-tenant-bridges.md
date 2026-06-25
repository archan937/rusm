# Multi-tenant bridges

A [custom bridge](/build-an-app/add-your-own-functions) is one shared host implementation,
but it doesn't have to treat every caller the same. A single `gql()` can connect with client
X's GraphQL credentials and client Y's — resolved **dynamically at call time** from your own
source of truth, not baked in at deploy.

The mechanism is easy; the part worth getting right is **which tenant the call belongs to**,
and how trustworthy that answer has to be.

## The capability profile gates access

Access is gated by the [capability profile](/build-an-app/grant-capabilities): a component
reaches `gql` only if its profile grants the bridge (`bridges = ["gql"]`), and the bridge
gates default-deny on `self.caps().allows_bridge("gql")`. That decides *who may call* — the
per-tenant credentials are the runtime concern below.

## Resolve the credentials dynamically

Whatever identifies the tenant, the bridge looks the credentials up **at the moment of the
call** — a `kv` read, a database query, an internal credentials service — never a static
env var:

```rust
struct Credentials {
    endpoint: String,
    token: String,
}

/// Your dynamic lookup: resolve `tenant` against whatever owns per-tenant config. Runs
/// host-side; no guest ever sees it. Cache it if you like — the point is it's resolved now.
async fn resolve_credentials(tenant: &str) -> Result<Credentials, String> {
    // …look up `tenant` → endpoint + token…
    todo!()
}
```

The only open question is where `tenant` comes from.

## Pattern A — the caller passes its tenant (Rust, TypeScript, or Go host)

Make the tenant an argument of the bridge call. The component states which tenant it's
acting for; the bridge resolves that tenant's credentials. This works for every host
language (it's just a call argument) and needs nothing from the runtime:

```rust
// bridges/gql/bridge.wit:  query: func(tenant: string, q: string) -> result<string, string>;
impl gql::Host for BridgeHost {
    async fn query(&mut self, tenant: String, q: String) -> Result<String, String> {
        let creds = resolve_credentials(&tenant).await?;
        run_query(&creds.endpoint, &creds.token, &q).await
    }
}
```

```ts
// the guest, in any language, just names its tenant:
await gql.query("client-x", "{ orders { id } }");
```

## Pattern B — the bridge reads the caller's identity (Rust host)

If you'd rather not thread a tenant argument, a **Rust** host bridge can read it from the
caller. `BridgeHost` hands you `self.pid()`, `self.runtime()`, and `self.caps()` on every
call; map the caller to a tenant via a name it registers under an explicit convention (not
"whatever name it happens to hold"):

```rust
use rusm_otp::Pid;   // add `rusm-otp` to the bridge crate to name Pid

impl gql::Host for BridgeHost {
    async fn query(&mut self, q: String) -> Result<String, String> {
        let pid = Pid::from_raw(self.pid());
        // A deliberate `tenant:` registry convention — discovery names are not identity.
        let tenant = self
            .runtime()
            .info(pid)
            .into_iter()
            .flat_map(|info| info.names)
            .find_map(|name| name.strip_prefix("tenant:").map(str::to_owned))
            .ok_or("caller registered no tenant")?;
        let creds = resolve_credentials(&tenant).await?;
        run_query(&creds.endpoint, &creds.token, &q).await
    }
}
```

```ts
import { Process } from "rusm-ts";
Process.register("tenant:client-x"); // once, at startup
```

A `host.ts` / `host.go` bridge can't do this — it runs behind a generated Rust shim and its
function receives only the call's arguments, not the caller. Use Pattern A there.

## The trust model — read this before using it for secrets

Both patterns are **guest-asserted**: the component chooses the tenant it claims, whether by
the argument it passes or the name it registers. That is exactly right when the components
are **your own**, deployed one-per-tenant and trusted to state their own identity — the
common multi-tenant-app case.

It is **not** an isolation boundary against a hostile component: nothing stops one from
claiming another tenant and resolving its credentials. For credentials that must be
unreachable across mutually-distrusting tenants, the identity has to be **unforgeable** —
established by the operator, outside the guest's control. A bridge today sees the caller's
`pid` and capability grant-flags but not its operator-declared identity, so hard isolation
means binding pid → tenant authoritatively where you spawn the components (the
[embedding](/deep-dive/embedding-rusm-as-a-library) model), rather than trusting a value the
guest supplies. Pick the pattern that matches your threat model — don't reach for B's
convenience when you actually need that boundary.
