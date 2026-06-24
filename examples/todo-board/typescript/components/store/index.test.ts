import { test, expect, beforeEach } from "bun:test";

// Unit tests for the store service: an in-memory `kv` + a capturing `Process` (records the
// publish) stand in for the host globals (re-pointed per-test, cross-file-safe).
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
const store = await import("./index");

beforeEach(() => {
  buckets.clear();
  sent.length = 0;
  subscribers = [];
  Object.assign(kv as object, mockKv());
  Object.assign(Process as object, mockProcess());
});

test("add creates a todo and publishes the list to subscribers", () => {
  subscribers = [7n];
  expect(store.add({ text: "hi" })).toEqual({ id: 1, text: "hi", done: false });
  expect(store.list()).toEqual([{ id: 1, text: "hi", done: false }]);
  expect(sent).toHaveLength(1); // published to the one subscriber
});

test("toggle flips done; null for an unknown id", () => {
  store.add({ text: "a" });
  expect(store.toggle(1)?.done).toBe(true);
  expect(store.toggle(1)?.done).toBe(false);
  expect(store.toggle(99)).toBeNull();
});

test("remove deletes; false for an unknown id", () => {
  store.add({ text: "a" });
  expect(store.remove(1)).toBe(true);
  expect(store.remove(1)).toBe(false);
});

test("all() streams each todo", async () => {
  store.add({ text: "a" });
  store.add({ text: "b" });
  const got: string[] = [];
  for await (const t of store.all()) got.push(t.text);
  expect(got).toEqual(["a", "b"]);
});

test("importMany adds every todo and reports progress per item", async () => {
  const progress: number[] = [];
  const n = await store.importMany(["x", "y"], (done) => progress.push(done));
  expect(n).toBe(2);
  expect(progress).toEqual([1, 2]);
  expect(store.list().map((t) => t.text)).toEqual(["x", "y"]);
});
