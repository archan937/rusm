// bridges/mailer/host.ts — the ONLY file you write for a TypeScript mailer bridge.
// rusm build generates the Rust delegation shim, the TS dispatch runner (_runner.ts),
// and all host crate glue. Bun bundles the runner to wasm/bridge-mailer.js.
//
// The runner is a long-lived resident actor: the API key loads once at startup and is
// available for the node's entire lifetime. Set RESEND_API_KEY in your .env.
const API_KEY = process.env.RESEND_API_KEY ?? "";

export async function send(msg: { to: string; subject: string; body: string }): Promise<boolean> {
  const res = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      from: "noreply@example.com",
      to: msg.to,
      subject: msg.subject,
      html: msg.body,
    }),
  });
  return res.ok;
}
