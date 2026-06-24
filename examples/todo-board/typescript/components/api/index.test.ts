import { test, expect, beforeEach } from "bun:test";

// Unit tests for the todo HTTP API: an in-memory `kv` and a capturing `Process` stand in
// for the host globals (installed before rusm-ts captures them; re-pointed per-test so a
// sibling test file that captured the shared globals first can't bleed in). We drive the
// real `fetch` handler with real `Request`s and assert the CRUD + publish behaviour.
const buckets = new Map<string, Map<string, Uint8Array>>();
const sent: Array<[bigint, string | Uint8Array]> = [];
let subscribers: bigint[] = [];
const enc = new TextEncoder();

const mockKv = () => ({
  bucket(name: string) {
    const b = buckets.get(name) ?? buckets.set(name, new Map()).get(name)!;
    return {
      get: (k: string) => b.get(k) ?? null,
      set: (k: string, v: string | Uint8Array) =>
        void b.set(k, typeof v === "string" ? enc.encode(v) : v),
      delete: (k: string) => b.delete(k),
      exists: (k: string) => b.has(k),
      list: () => [...b.keys()],
    };
  },
});
const mockProcess = () => ({
  self: () => 1n,
  whereisTag: (_tag: string) => subscribers,
  send: (to: bigint, msg: string | Uint8Array) => void sent.push([to, msg]),
  registerTag: (_tag: string) => {},
});
(globalThis as unknown as { kv: unknown }).kv = mockKv();
(globalThis as unknown as { Process: unknown }).Process = mockProcess();

const { kv, Process } = await import("rusm-ts");
const handle = (await import("./index")).default;

beforeEach(() => {
  buckets.clear();
  sent.length = 0;
  subscribers = [];
  Object.assign(kv as object, mockKv());
  Object.assign(Process as object, mockProcess());
});

const req = (method: string, path: string, body?: unknown) =>
  new Request(`http://api${path}`, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
const create = (text: string) => handle(req("POST", "/todos", { text }));

test("GET /todos lists todos by id, empty at first", async () => {
  expect(await (await handle(req("GET", "/todos"))).json()).toEqual([]);
  await create("first");
  await create("second");
  const list = await (await handle(req("GET", "/todos"))).json();
  expect(list).toEqual([
    { id: 1, text: "first", done: false },
    { id: 2, text: "second", done: false },
  ]);
});

test("POST /todos creates a todo (201) and publishes the new list to subscribers", async () => {
  subscribers = [42n, 43n];
  const res = await create("buy milk");
  expect(res.status).toBe(201);
  expect(await res.json()).toEqual({ id: 1, text: "buy milk", done: false });
  // The change fanned out to every subscriber (the feed) once each.
  expect(sent.map(([pid]) => pid)).toEqual([42n, 43n]);
  expect(JSON.parse(new TextDecoder().decode(sent[0][1] as Uint8Array))).toEqual([
    { id: 1, text: "buy milk", done: false },
  ]);
});

test("POST /todos rejects an empty text with 400 and publishes nothing", async () => {
  subscribers = [42n];
  expect((await create("   ")).status).toBe(400);
  expect((await handle(req("POST", "/todos", {}))).status).toBe(400);
  expect(sent).toEqual([]);
});

test("PATCH /todos/:id toggles done; 404 for an unknown id", async () => {
  await create("task");
  const toggled = await handle(req("PATCH", "/todos/1"));
  expect(toggled.status).toBe(200);
  expect((await toggled.json()).done).toBe(true);
  expect((await (await handle(req("PATCH", "/todos/1"))).json()).done).toBe(false);
  expect((await handle(req("PATCH", "/todos/99"))).status).toBe(404);
});

test("DELETE /todos/:id removes a todo (204); 404 for an unknown id", async () => {
  await create("task");
  expect((await handle(req("DELETE", "/todos/1"))).status).toBe(204);
  expect(await (await handle(req("GET", "/todos"))).json()).toEqual([]);
  expect((await handle(req("DELETE", "/todos/1"))).status).toBe(404);
});

test("GET / serves the explanatory HTML page", async () => {
  const res = await handle(req("GET", "/"));
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toContain("text/html");
  const body = await res.text();
  expect(body).toContain("RUSM todo board");
  expect(body).toContain("Live feed"); // explains what each part showcases
});

test("OPTIONS is a CORS preflight; an unknown route is 404", async () => {
  const pre = await handle(req("OPTIONS", "/todos"));
  expect(pre.status).toBe(204);
  expect(pre.headers.get("access-control-allow-origin")).toBe("*");
  expect((await handle(req("GET", "/nope"))).status).toBe(404);
});
