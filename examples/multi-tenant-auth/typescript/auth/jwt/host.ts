// auth/jwt/host.ts — the host-authoritative auth hook for this listener.
//
// It runs BEFORE the per-connection handler is spawned: it validates the request and derives
// the tenant (`app_id`), which becomes the connection's host-only claims context. A valid
// request seeds that context; anything else is denied → the host refuses the upgrade with 401
// and no handler runs (fail-closed). Guest code never runs here and never sees these claims.
//
// This is a DUMMY verification for the example. A real hook would verify a JWT signature
// against the issuer's JWKS endpoint (the hook is async precisely so it can await that). A
// browser can't set `Authorization` on a WebSocket, so the token arrives as `?token=…`.

const TOKENS: Record<string, string> = {
  "acme-secret": "acme",
  "globex-secret": "globex",
};

function queryToken(query: string): string | undefined {
  for (const pair of query.split("&")) {
    const [k, v] = pair.split("=");
    if (k === "token") return v;
  }
  return undefined;
}

export async function authenticate(req: {
  method: string;
  path: string;
  query: string;
  headers: [string, string][];
}) {
  const header = req.headers.find(([k]) => k.toLowerCase() === "authorization")?.[1];
  const token = header?.replace(/^Bearer\s+/i, "") ?? queryToken(req.query);
  const appId = token ? TOKENS[token] : undefined;
  return appId ? { allow: { app_id: appId } } : { deny: true };
}
