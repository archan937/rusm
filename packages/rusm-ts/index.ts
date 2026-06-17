// rusm — the guest API for RUSM TypeScript components.
//
// The js-runner injects the `Process` actor API and the `spawn` typed-client
// factory as globals (and polyfills the Web APIs); this package re-exports them as
// a normal module and ships the types, so a component writes:
//
//     import { Process, spawn } from "rusm-ts";
//
// Pids are u64s, too large for a JS number, so they cross as `bigint`. Messages
// and stream chunks are `Uint8Array`. `receive` / `Stream.read` are async.

/** A back-pressured byte stream to or from another process. */
export interface Stream {
  /** Write a chunk; a string is sent as UTF-8. Returns false if the peer is gone. */
  write(chunk: string | Uint8Array): boolean;
  /** The next chunk, or `null` at end-of-stream. */
  read(): Promise<Uint8Array | null>;
  /** Close the stream (signals end-of-stream to the reader). */
  close(): void;
}

/** The RUSM actor API: this process and its peers. Mirrors the Erlang `Process`. */
export interface ProcessApi {
  self(): bigint;
  list(): bigint[];
  /** Spawn a registered component by name → its pid (capability-gated). With a runtime
   *  `source` (`inline:<js>` / `kv:<bucket>/<key>` / `url:`/`http(s)://…`), spawns a
   *  dynamic JS instance of a runner template — the loaded JS runs under the template's
   *  declared profile (you choose the code, the operator the capabilities). */
  spawn(name: string, source?: string): bigint;
  /** Monitor a process: its death arrives as a `{ __down }` message. */
  monitor(pid: bigint | string): void;
  send(to: bigint | string, msg: string | Uint8Array): void;
  /**
   * The next message as bytes. With `timeoutMs`, it's Erlang's `receive … after`:
   * resolves to `null` if the deadline passes before a message arrives — the basis
   * for an SSE heartbeat (wait for the next event *or* the tick).
   */
  receive(): Promise<Uint8Array>;
  receive(timeoutMs: number): Promise<Uint8Array | null>;
  /** The next message decoded as UTF-8 (`null` on `timeoutMs` timeout). */
  receiveText(): Promise<string>;
  receiveText(timeoutMs: number): Promise<string | null>;
  register(name: string): boolean;
  whereis(name: string): bigint | null;
  isAlive(pid: bigint | string): boolean;
  kill(pid: bigint | string): boolean;
  setLabel(label: string): void;
  /** Join **this** process to a process-group `tag` (Erlang's `pg`); released on exit. */
  registerTag(tag: string): void;
  /** Leave a process-group `tag` this process holds. */
  unregisterTag(tag: string): void;
  /** Live members (pids) of process-group `tag`. */
  whereisTag(tag: string): bigint[];
  /** Terminate every live member of `tag`; returns the count. Needs `process-control`. */
  killTag(tag: string): number;
  openStream(to: bigint | string): Stream | null;
  acceptStream(): Stream;
}

/** The result of a typed call: `await` it for the reply, or `for await` it to
 *  stream a generator handler's chunks. Function arguments become callbacks that
 *  stay in the caller — the service's invocations travel back as messages. */
export type RusmCall<R> =
  R extends AsyncIterable<infer T>
    ? AsyncIterable<T> & PromiseLike<void>
    : R extends Iterable<infer T>
      ? AsyncIterable<T> & PromiseLike<void>
      : Promise<Awaited<R>>;

/** A typed client over a spawned service: each exported function becomes a call
 *  (`await`) — or a stream (`for await`); `cast` is fire-and-forget. */
export type ServiceClient<T> = {
  [K in keyof T]: T[K] extends (...args: infer A) => infer R
    ? (...args: A) => RusmCall<R>
    : never;
} & {
  readonly cast: {
    [K in keyof T]: T[K] extends (...args: infer A) => any
      ? (...args: A) => void
      : never;
  };
  readonly pid: bigint;
  stop(): void;
};

/** How a supervisor reacts when one child dies. */
export type Strategy = "one_for_one" | "one_for_all" | "rest_for_one";

/** Options for [`supervise`]: which children (registered component names) to run,
 *  the restart strategy, and an optional restart ceiling (overload protection). */
export interface SupervisorOptions {
  children: string[];
  strategy?: Strategy;
  /** Give up after this many restarts (0 = never). By default counted over the
   *  supervisor's whole lifetime; set {@link maxSeconds} for a sliding window. */
  maxRestarts?: number;
  /** Restart-intensity window in seconds: give up only if more than `maxRestarts`
   *  happen within this span (Erlang's `{max_restarts, max_seconds}`). Without it,
   *  `maxRestarts` counts over the whole lifetime. */
  maxSeconds?: number;
}

/** One namespace in the node's durable key-value store (gated by the `storage`
 *  capability). Values are bytes; `set` also accepts a string (UTF-8). A denied or
 *  failed op throws. See {@link kv}. */
export interface KvBucket {
  /** The stored value, or `null` if absent. */
  get(key: string): Uint8Array | null;
  set(key: string, value: string | Uint8Array): void;
  /** Remove `key`; returns whether it existed. */
  delete(key: string): boolean;
  exists(key: string): boolean;
  /** Every key in this bucket, sorted. */
  list(): string[];
}

/** Durable, embedded key-value storage — the node owns one store; guests granted
 *  the `storage` capability open buckets within it. */
export interface Kv {
  bucket(name: string): KvBucket;
}

// The runner installs these globals before the bundle runs (and wraps the bundle
// in a CommonJS scope, so this module's bindings never clobber them).
const g = globalThis as unknown as {
  Process: ProcessApi;
  spawn: <T>(component: string) => ServiceClient<T>;
  supervise: (opts: SupervisorOptions) => Promise<void>;
  kv: Kv;
};

/** The actor API for this process. */
export const Process: ProcessApi = g.Process;

/** The node's durable key-value store (gated by the `storage` capability). */
export const kv: Kv = g.kv;

/** Spawn a registered component and get a typed client — the concealed function
 *  call (spawn + send + receive, hidden). Type it with the service's published
 *  contract: `import type { Calc } from "../calc"` then `spawn<Calc>("calc")`. */
export const spawn = <T = Record<string, (...args: any[]) => any>>(
  component: string,
): ServiceClient<T> => g.spawn<T>(component);

/** Run a **supervisor**: spawn + monitor the given child components and restart
 *  them per the strategy when one dies. `await` it as your worker's body. */
export const supervise = (opts: SupervisorOptions): Promise<void> =>
  g.supervise(opts);

/** One live WebSocket connection. Reply to it with {@link Socket.send}; `id` is its
 *  writer pid, should you want to address it directly (e.g. a registry of peers). */
export interface Socket {
  readonly id: bigint;
  /** Send one frame back to this connection (a string is sent as UTF-8). */
  send(data: string | Uint8Array): void;
}

/** Per-connection WebSocket handlers — the clean shape behind {@link websocket}. */
export interface WebSocketHandlers {
  /** A connection opened. */
  open?(socket: Socket): void;
  /** One inbound frame from a connection. */
  message(socket: Socket, data: Uint8Array): void;
  /** A connection closed. */
  close?(socket: Socket): void;
}

/** Build a WebSocket component from per-connection handlers — no pids, no message
 *  plumbing. Each connection is a {@link Socket} you reply to with `socket.send(…)`.
 *  Export the result as the component's default:
 *
 *  ```ts
 *  export default websocket({ message: (s, data) => s.send(data) }); // echo
 *  ```
 */
export const websocket = (handlers: WebSocketHandlers) => {
  const socket = (id: bigint): Socket => ({
    id,
    send: (data) => Process.send(id, data),
  });
  return {
    websocket: {
      open: (conn: bigint) => handlers.open?.(socket(conn)),
      message: (conn: bigint, data: Uint8Array) =>
        handlers.message(socket(conn), data),
      close: (conn: bigint) => handlers.close?.(socket(conn)),
    },
  };
};
