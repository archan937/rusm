// bridges/tenants/host.ts — the per-tenant bridge host.
//
// `rusm build` generates the Rust delegation shim, the TS runner, and the host binary. At
// runtime this runs as a resident actor; each call crosses the actor wire. The host forwards
// the caller's request context in-band, so `context()` here is the tenant the auth hook
// established — host-decided, never guest input.

import { context } from "rusm-ts";

// Stand-in for a per-tenant backend. In a real app `data()` would use THIS tenant's own
// credentials to query THIS tenant's database/API — the whole point of a multi-tenant bridge.
const TENANT_DATA: Record<string, string> = {
  acme: "Acme Corp — 3 open orders",
  globex: "Globex Inc — 7 open orders",
};

export function data(): string {
  const appId = context().app_id; // host-decided tenant; the guest cannot set or forge this
  return TENANT_DATA[appId] ?? `unknown tenant: ${appId}`;
}
