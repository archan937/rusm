import { test, expect, beforeEach } from "bun:test";

// The helper captures `Process` at import (a single cached module), so we install a mock
// before importing, then re-point `Process.send` to this file's sink in `beforeEach` —
// otherwise a sibling test file that mocks the same global would steal our sends.
const sent: Array<[bigint, string | Uint8Array]> = [];
(globalThis as unknown as { Process: unknown }).Process = {
  send: (to: bigint, msg: string | Uint8Array) => sent.push([to, msg]),
};

const { sse, Process } = await import("./index");

beforeEach(() => {
  sent.length = 0;
  (Process as unknown as { send: unknown }).send = (
    to: bigint,
    msg: string | Uint8Array,
  ) => sent.push([to, msg]);
});

test("sse() exposes the { sse: { open, message, close, done } } shape the runner drives", () => {
  const handler = sse({ message: () => {} });
  expect(typeof handler.sse.open).toBe("function");
  expect(typeof handler.sse.message).toBe("function");
  expect(typeof handler.sse.close).toBe("function");
  expect(typeof handler.sse.done).toBe("function");
});

test("a connection event becomes an SseStream; stream.data routes to Process.send(conn, …)", () => {
  const opened: bigint[] = [];
  const closed: bigint[] = [];
  const handler = sse({
    open: (s) => opened.push(s.id),
    message: (s, ev) => s.data(ev), // emit the pushed event
    close: (s) => closed.push(s.id),
  });

  handler.sse.open(7n);
  expect(opened).toEqual([7n]);

  const event = new Uint8Array([1, 2, 3]);
  handler.sse.message(7n, event);
  expect(sent).toEqual([[7n, event]]);

  handler.sse.close(7n);
  expect(closed).toEqual([7n]);
});

test("stream.close() flips done() so the runner stops the stream (self-close)", () => {
  let stream!: { close(): void };
  const handler = sse({
    open: (s) => {
      stream = s;
    },
    message: () => {},
  });

  handler.sse.open(1n);
  expect(handler.sse.done()).toBe(false);
  stream.close();
  expect(handler.sse.done()).toBe(true);
});

test("open and close are optional — a message-only handler never throws on them", () => {
  const handler = sse({ message: () => {} });
  expect(() => handler.sse.open(1n)).not.toThrow();
  expect(() => handler.sse.close(1n)).not.toThrow();
});
