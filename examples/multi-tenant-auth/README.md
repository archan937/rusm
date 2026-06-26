# multi-tenant-auth

A **host-authoritative auth hook** establishes the tenant for each request; a **custom bridge**
then acts for that tenant — and the guest application code is auth-unaware throughout (it never
sees, sets, or forges the identity).

| Flavour | Auth hook | Bridge | Serves |
|---|---|---|---|
| [`typescript/`](./typescript/) | `auth/jwt/host.ts` | `bridges/tenants/host.ts` — reads `context()` | HTTP :8080 |

The same model works with a Rust or Go auth hook + bridge (`auth/<name>/host.{rs,go}`,
`bridges/<name>/host.{rs,go}`); this example ships the TypeScript flavour.

```sh
cd examples/multi-tenant-auth/typescript
bun install && rusm build && rusm serve
```

> Requires `rusm` / `rusm-ts` **≥ 0.7.0** (serving auth hooks + `context()`).

## Scaffold your own

```sh
rusm generate authentication jwt --lang ts     # auth/jwt/host.ts
rusm generate bridge tenants                    # bridges/tenants/host.*
# then add `authentication = "jwt"` to the [[serve]] listener
```

See [`docs/build-an-app/multi-tenant-bridges.md`](../../docs/build-an-app/multi-tenant-bridges.md)
for the full story and why it's secure by construction.
