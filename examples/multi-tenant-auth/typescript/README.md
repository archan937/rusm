# multi-tenant-auth — TypeScript

A **host-authoritative auth hook** + a **per-tenant bridge**. The guest is auth-unaware: it
never validates a token, never sees the claims, and never chooses a tenant. The host validates
the request, derives the identity, and the bridge acts on it.

```
request ─▶ auth/jwt (host) ─▶ components/api (guest) ─▶ bridges/tenants (host)
           verifies token       auth-unaware code         acts for the tenant
           → app_id claim   ───────────────────────────▶ reads context().app_id
           → or 401
```

- `auth/jwt/host.ts` — runs **before** the handler is spawned. Validates the request, derives
  `app_id`, returns `{ allow: { app_id } }` or `{ deny: true }`. A denial is `401` and no
  handler runs (fail-closed). A real hook would verify a JWT signature; this one maps a dummy
  token to a tenant.
- `bridges/tenants/host.ts` — reads `context().app_id` (the tenant the hook established for
  this request — host-decided, never guest input) and returns that tenant's data.
- `components/api/index.ts` — an HTTP `fetch` handler that just calls `tenants.data()`.

## Run

> Requires `rusm` and `rusm-ts` **≥ 0.7.0** (the release that adds serving auth hooks +
> `context()`).

```sh
bun install
rusm build
rusm serve
```

Then request as different tenants — same guest code, per-tenant data:

```sh
curl -H 'authorization: Bearer acme-secret'   127.0.0.1:8080/   # → Acme Corp — 3 open orders
curl -H 'authorization: Bearer globex-secret' 127.0.0.1:8080/   # → Globex Inc — 7 open orders
curl -i 127.0.0.1:8080/                                          # → 401 (no token)
curl -i -H 'authorization: Bearer nope' 127.0.0.1:8080/         # → 401 (bad token)
```

## How it works

`rusm build` discovers `auth/jwt/host.ts` and `bridges/tenants/host.ts`, generates the Rust
host crate (auth hook + bridge shim compiled in), the TS runners, and a per-app **js-http-runner**
with the bridge compiled in (so the TS `fetch` handler reaches the bridge). `authentication =
"jwt"` in `rusm.toml` applies the hook to the listener; the claims become the request's
host-only context, which the bridge reads via `context()`.

See [`../../../docs/build-an-app/multi-tenant-bridges.md`](../../../docs/build-an-app/multi-tenant-bridges.md)
for the full model and the Rust/Go variants.
