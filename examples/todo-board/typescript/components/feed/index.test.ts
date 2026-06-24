import { test, expect, beforeEach } from "bun:test";

// Unit tests for the SSE feed: a capturing `Process` (records the tag it subscribes to and
// every emitted event) and an in-memory `kv` (for the connect-time snapshot) stand in for
// the host globals. We drive the handler table the `sse({…})` helper produces.
const todos = new Map<string, Uint8Array>();
const sent: Array<[bigint, string | Uint8Array]> = [];
const tags: string[] = [];
const enc = new TextEncoder();

const mockKv = () => ({
  bucket: (_name: string) => ({
    get: (k: string) => todos.get(k) ?? null,
    set: (k: string, v: string | Uint8Array) =>
      void todos.set(k, typeof v === "string" ? enc.encode(v) : v),
    delete: (k: string) => todos.delete(k),
    exists: (k: string) => todos.has(k),
    list: () => [...todos.keys()],
  }),
});
const mockProcess = () => ({
  self: () => 1n,
  whereisTag: (_tag: string) => [] as bigint[],
  send: (to: bigint, msg: string | Uint8Array) => void sent.push([to, msg]),
  registerTag: (tag: string) => void tags.push(tag),
});
(globalThis as unknown as { kv: unknown }).kv = mockKv();
(globalThis as unknown as { Process: unknown }).Process = mockProcess();

const { kv, Process } = await import("rusm-ts");
const feed = (await import("./index")).default;

beforeEach(() => {
  todos.clear();
  sent.length = 0;
  tags.length = 0;
  Object.assign(kv as object, mockKv());
  Object.assign(Process as object, mockProcess());
});

test("on connect, subscribes to the todo tag and emits the current list as the first event", () => {
  todos.set("1", enc.encode(JSON.stringify({ id: 1, text: "a", done: false })));
  feed.sse.open(7n);
  expect(tags).toEqual(["todos"]); // subscribed
  expect(sent).toHaveLength(1);
  const [pid, payload] = sent[0];
  expect(pid).toBe(7n);
  expect(JSON.parse(payload as string)).toEqual([{ id: 1, text: "a", done: false }]);
});

test("a published change is emitted to the client verbatim", () => {
  const event = enc.encode(JSON.stringify([{ id: 1, text: "a", done: true }]));
  feed.sse.message(7n, event);
  expect(sent).toEqual([[7n, event]]);
});

test("open/close are wired and the stream does not self-close", () => {
  expect(() => feed.sse.open(1n)).not.toThrow();
  expect(() => feed.sse.close(1n)).not.toThrow();
  expect(feed.sse.done()).toBe(false);
});
