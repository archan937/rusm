// components/api/index.ts — a self-routing fetch handler over durable kv.
import { kv } from "rusm-ts";

const links = () => kv.bucket("links");

export default async function handle(req: Request): Promise<Response> {
  const url = new URL(req.url);

  // POST /shorten — the body is the long URL; store it under a fresh code.
  if (req.method === "POST" && url.pathname === "/shorten") {
    const target = (await req.text()).trim();
    if (!target) return new Response("send a URL in the body\n", { status: 400 });
    const code = String(links().list().length + 1); // a simple sequential code
    links().set(code, target);
    return new Response(`/${code}\n`, { status: 201 });
  }

  // GET /:code — look the code up and redirect to the URL.
  const stored = links().get(url.pathname.slice(1));
  if (stored) {
    const location = new TextDecoder().decode(stored);
    return new Response(null, { status: 302, headers: { location } });
  }

  return new Response("not found\n", { status: 404 });
}
