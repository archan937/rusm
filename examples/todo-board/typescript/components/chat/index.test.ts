import { test, expect, beforeEach } from "bun:test";

// Unit tests for the chat room protocol: a capturing `Process` records every `send` and
// the tag joined, and `whereisTag` returns a configurable room membership. We drive the
// handler table the `websocket({…})` helper produces.
//
// NOTE: `room` is module-level state — in production that's *per connection* (one process
// per socket), but in this single-module test it persists across cases, so the "before
// joining" case runs first (a fresh module has no room) and the join case sets it for the
// cases after.
const sent: Array<[bigint, string]> = [];
const tags: string[] = [];
let members: bigint[] = [];
const dec = new TextDecoder();

const mockProcess = () => ({
  self: () => 1n,
  whereisTag: (_tag: string) => members,
  send: (to: bigint, msg: string | Uint8Array) =>
    void sent.push([to, typeof msg === "string" ? msg : dec.decode(msg)]),
  registerTag: (tag: string) => void tags.push(tag),
});
// rusm-ts captures `kv` + `Process` at first import; set both before importing so the
// shared capture is never undefined, whichever test file loads first (chat ignores `kv`).
(globalThis as unknown as { kv: unknown }).kv = { bucket: () => ({}) };
(globalThis as unknown as { Process: unknown }).Process = mockProcess();

const { Process } = await import("rusm-ts");
const chat = (await import("./index")).default;

beforeEach(() => {
  sent.length = 0;
  tags.length = 0;
  members = [];
  Object.assign(Process as object, mockProcess());
});

const frame = (obj: unknown) => new TextEncoder().encode(JSON.stringify(obj));
const sentTo = (pid: bigint) => sent.filter(([to]) => to === pid).map(([, m]) => JSON.parse(m));

test("on connect, greets with a join prompt", () => {
  chat.websocket.open(7n);
  expect(sentTo(7n)).toEqual([{ system: 'connected — send {"join":"<room>"} to join a room' }]);
});

test("saying before joining is rejected (still no room on a fresh connection)", () => {
  chat.websocket.message(7n, frame({ say: "hi" }));
  expect(sentTo(7n)).toEqual([{ system: "join a room first" }]);
});

test("joining tags the room, welcomes the joiner, and announces to the others only", () => {
  members = [1n, 2n]; // self (1) + an existing member (2)
  chat.websocket.message(1n, frame({ join: "general" }));
  expect(tags).toEqual(["room:general"]);
  expect(sentTo(1n)).toEqual([{ system: "welcome to #general" }]); // joiner: welcome only
  expect(sentTo(2n)).toEqual([{ from: "system", text: "a new member joined #general" }]); // others: announce
});

test("saying fans the message out to every room member, including the sender (echo)", () => {
  members = [1n, 2n];
  chat.websocket.message(1n, frame({ say: "hello" }));
  const relay = { from: "1", text: "hello" };
  expect(sentTo(1n)).toEqual([relay]); // sender sees their own message
  expect(sentTo(2n)).toEqual([relay]); // and so does the peer
});

test("a peer's relay is forwarded to this client", () => {
  const relay = { from: "9", text: "hey there" };
  chat.websocket.message(7n, frame(relay));
  expect(sentTo(7n)).toEqual([relay]);
});

test("open/close are wired and never throw", () => {
  expect(() => chat.websocket.open(1n)).not.toThrow();
  expect(() => chat.websocket.close(1n)).not.toThrow();
});
