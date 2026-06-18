// The todo HTTP API — a web-standard `fetch` handler (it does its own routing; HTTP
// listeners need no [serve.routes] for TS). Each request runs in a fresh, isolated WASM
// instance. Reads/writes the durable todo list and publishes every change to the feed's
// subscribers. The data layer lives in ../../lib/todos (shared with the feed).
import * as todos from "../../lib/todos";

const cors = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, PATCH, DELETE, OPTIONS",
  "access-control-allow-headers": "content-type",
};

const json = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...cors },
  });

export default async function handle(request: Request): Promise<Response> {
  const url = new URL(request.url);
  const segments = url.pathname.split("/").filter(Boolean); // "/todos/3" → ["todos", "3"]
  const id = segments[1] !== undefined ? Number(segments[1]) : null;

  // CORS preflight — so a browser app on another origin can talk to the API.
  if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: cors });

  // GET /todos — the whole list.
  if (request.method === "GET" && url.pathname === "/todos") return json(todos.list());

  // POST /todos — add one ({ "text": "..." }).
  if (request.method === "POST" && url.pathname === "/todos") {
    const body = (await request.json().catch(() => ({}))) as { text?: string };
    const text = body.text?.trim();
    if (!text) return json({ error: "text is required" }, 400);
    const todo: todos.Todo = { id: todos.nextId(), text, done: false };
    todos.save(todo);
    console.log(`created #${todo.id}: ${todo.text}`);
    todos.publish();
    return json(todo, 201);
  }

  // PATCH /todos/:id — toggle done.
  if (request.method === "PATCH" && id !== null) {
    const todo = todos.get(id);
    if (!todo) return json({ error: "no such todo" }, 404);
    todo.done = !todo.done;
    todos.save(todo);
    console.log(`toggled #${id} → ${todo.done ? "done" : "open"}`);
    todos.publish();
    return json(todo);
  }

  // DELETE /todos/:id — remove.
  if (request.method === "DELETE" && id !== null) {
    if (!todos.remove(id)) return json({ error: "no such todo" }, 404);
    console.log(`deleted #${id}`);
    todos.publish();
    return new Response(null, { status: 204, headers: cors });
  }

  return json({ error: "not found" }, 404);
}
