// A TypeScript HTTP handler that calls the mailer bridge — proving a TS guest reaches a
// native bridge. `mailer` is the typed global the per-app js-runner exposes (rusm build
// compiles it in); bridge calls are synchronous from the guest's perspective.
/// <reference path="../../bridges.d.ts" />
import { http } from "rusm-ts";

export default http({
  async post(req) {
    const { to, subject, body } = await req.json();
    const sent = mailer.send({ to, subject, body });
    return sent
      ? new Response("queued", { status: 202 })
      : new Response("delivery failed", { status: 502 });
  },
});
