import { test, expect, beforeEach } from "bun:test";

// Unit test for the reporter worker: a fake typed client stands in for `spawn<Store>`, so
// we can assert the worker exercises the whole composition surface — call, callback,
// streaming, cast — and then parks. (`Process.receive` is mocked to resolve so the park
// doesn't hang the test.)
const calls: string[] = [];
let storeTodos: Array<{ id: number; text: string; done: boolean }> = [];

const fakeStore = {
  list: async () => {
    calls.push("call:list");
    return storeTodos;
  },
  importMany: async (texts: string[], onProgress: (done: number) => void) => {
    calls.push("callback:importMany");
    texts.forEach((_, i) => onProgress(i + 1));
    storeTodos = texts.map((text, i) => ({ id: i + 1, text, done: false }));
    return texts.length;
  },
  all: async function* () {
    calls.push("stream:all");
    for (const t of storeTodos) yield t;
  },
  cast: { ping: () => calls.push("cast:ping") },
};
const mockProcess = () => ({ receive: async () => calls.push("park") });
(globalThis as unknown as { spawn: unknown }).spawn = (_name: string) => fakeStore;
(globalThis as unknown as { Process: unknown }).Process = mockProcess();

const { Process } = await import("rusm-ts");
const reporter = (await import("./index")).default;

beforeEach(() => {
  calls.length = 0;
  storeTodos = [];
  (globalThis as unknown as { spawn: unknown }).spawn = (_name: string) => fakeStore;
  Object.assign(Process as object, mockProcess());
});

test("exercises call, callback, streaming, and cast — then parks", async () => {
  await reporter();
  // On an empty board: summary (call), seed (callback), stream the result, fire a cast,
  // then park to stay resident.
  expect(calls).toEqual([
    "call:list",
    "callback:importMany",
    "stream:all",
    "cast:ping",
    "park",
  ]);
});

test("does not re-seed when the board already has todos", async () => {
  storeTodos = [{ id: 1, text: "existing", done: false }];
  await reporter();
  expect(calls).not.toContain("callback:importMany");
  expect(calls).toEqual(["call:list", "stream:all", "cast:ping", "park"]);
});
