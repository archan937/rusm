# Multi-tenant bridges

A [custom bridge](/build-an-app/add-your-own-functions) is one shared host implementation, but
it doesn't have to treat every caller the same. A single `gql()` can connect to client X's
GraphQL backend with X's credentials and client Y's with Y's — and the part that decides *which
tenant a request belongs to* is **host-authoritative**: established by the operator from a
validated request, never asserted by the guest.

The guest application code is **auth-unaware**. It never sees, sets, or forges the tenant. The
host validates the request, derives the identity, and the bridge acts on it.

## The shape

```
request ──▶ auth hook (host) ──▶ handler (guest) ──▶ bridge (host)
            validates token       runs your code      acts for the tenant
            → claims context  ───────────────────────▶ reads the claims
            → or 401
```

Two host-side pieces, one guest in the middle that knows nothing:

1. An **auth hook** validates the incoming request (a JWT in a header, a token in a query
   param) and produces **claims** — e.g. `app_id = "acme"` — or rejects it with `401`.
2. The claims become the request's **host-only context**. It rides every message through the
   call graph (handler → any sub-component it spawns and calls → the bridge), so the bridge
   reads the tenant with `context()`.

## The auth hook

An auth hook is host code at `auth/<name>/host.{rs,ts,go}` — scaffold one with
[`rusm generate authentication`](/build-an-app/the-rusm-cli). It runs **before** the handler is
spawned: a valid request seeds the claims context; an invalid one is `401` and no handler runs
(fail-closed). Apply it to a listener in `rusm.toml`:

```toml
[[serve]]
protocol = "http"
listen = "127.0.0.1:8080"
authentication = "jwt"        # runs auth/jwt/host.* before every request on this listener

[serve.routes]
"GET /orders" = "orders#list"
```

A browser can't set `Authorization` on a WebSocket, so for `ws`/`sse` the token usually arrives
as a query param — the hook sees both headers and the query.

::: code-group

```rust [auth/jwt/host.rs]
use rusm_wasm::{AuthRequest, AuthVerdict};

pub async fn authenticate(req: AuthRequest) -> AuthVerdict {
    match req.header("authorization").and_then(verify) {
        Some(app_id) => AuthVerdict::Allow(vec![("app_id".into(), app_id)]),
        None => AuthVerdict::Deny, // → 401, the handler never runs
    }
}
```

```ts [auth/jwt/host.ts]
// req = { method, path, query, headers: [name, value][] }
export async function authenticate(req) {
  const auth = req.headers.find(([k]) => k.toLowerCase() === "authorization")?.[1];
  const appId = verify(auth);
  return appId ? { allow: { app_id: appId } } : { deny: true };
}
```

```go [auth/jwt/host.go]
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func Authenticate(req rusm.AuthRequest) rusm.AuthVerdict {
	if appID, ok := verify(req.Header("authorization")); ok {
		return rusm.Allow(map[string]string{"app_id": appID})
	}
	return rusm.Deny() // → 401
}
```

:::

A Rust hook compiles into the node's host binary (zero overhead). A TS/Go hook runs as a
**resident dispatch runner**; the host round-trips each request to it. Either way, if the hook
is missing, crashes, or returns anything that isn't an explicit allow, the request is **denied** —
a broken hook never lets one through.

## The bridge reads the tenant

The claims context reaches the bridge with no cooperation from the guest — a bridge reads it
through `context()`, in every host language:

::: code-group

```rust [bridges/gql/host.rs]
impl gql::Host for BridgeHost {
    async fn query(&mut self, q: String) -> Result<String, String> {
        // The tenant the auth hook established for this request — host-decided, not guest input.
        let app_id = self.context().get("app_id").ok_or("no tenant")?;
        let creds = credentials_for(app_id).await?;     // per-tenant, resolved now
        run_query(&creds.endpoint, &creds.token, &q).await
    }
}
```

```ts [bridges/gql/host.ts]
import { context } from "rusm-ts";

export async function query(q: string): Promise<string> {
  const appId = context().app_id;                 // host-decided tenant
  const creds = await credentialsFor(appId);
  return runQuery(creds.endpoint, creds.token, q);
}
```

```go [bridges/gql/host.go]
func Query(q string) string {
	appID := rusm.Context()["app_id"]               // host-decided tenant
	creds := credentialsFor(appID)
	return runQuery(creds.endpoint, creds.token, q)
}
```

:::

Client X's component and client Y's run the **same** `query("{ orders { id } }")`; the bridge
connects to different backends because `context()` differs — and the components can't change
that. (For a TS/Go bridge the host forwards the caller's context to the bridge's runner in-band;
a non-bridge component's `context()` is empty, so guest application code still never sees it.)

## Why it's secure — by construction

This is not a runtime check you can forget; it's structural:

- **No guest op.** There is no WIT function to read, write, or forge the context. A guest
  literally cannot reach it (a build-failing test guards that the actor world exposes none).
- **Host-sourced.** The context is filled only by the auth hook (host code); a guest supplies
  bytes, never the identity.
- **Carried out-of-band.** It rides *beside* each message as opaque metadata, never inside the
  payload — and is re-bound to its own request on every receive, so a shared resident handling
  many tenants never leaks one tenant's identity into another's call.
- **Per-request isolation.** Serving is process-per-request / process-per-connection: a fresh,
  isolated instance per unit of work, dropped after.

The guest may still *see* the raw request headers (handlers need them), but seeing the token
doesn't let it pick a tenant — the bridge reads the host-only context, never guest input.

Everything here works in all three host languages — the auth hook (`auth/<name>/host.{rs,ts,go}`)
and the bridge that reads `context()` — so a multi-tenant `gql` bridge can be authored in Rust,
TypeScript, or Go.
