// components/api/index.ts — the guest handler. It is AUTH-UNAWARE: it never validates a
// token, never sees the claims, and never chooses a tenant. It just calls the bridge, which
// acts for whatever tenant the auth hook established for THIS request. Two clients run the
// exact same code and get their own data because the host-only context differs — and neither
// can change that.
/// <reference path="../../bridges.d.ts" />

export default async function handle(_req: Request): Promise<Response> {
  return new Response(`${tenants.data()}\n`);
}
