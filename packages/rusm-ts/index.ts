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
  /** Send a **text** WebSocket frame on this connection (binary frames are a plain
   *  {@link ProcessApi.send} to the writer pid). `false` if not a WS handler / socket closed. */
  sendText(text: string): boolean;
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
  /** The raw per-connection serving context (a WebSocket/SSE handler's request), or
   *  `null` for any other process. Prefer {@link Socket.info} / {@link SseStream.info},
   *  which wrap it with `param`/`header` lookups. */
  connection(): ConnectionInfoData | null;
}

/** The raw per-connection request context the runtime exposes — wrapped ergonomically by
 *  {@link Socket.info} / {@link SseStream.info}. */
export interface ConnectionInfoData {
  readonly method: string;
  readonly path: string;
  readonly query: string;
  /** Route params captured from the listener's `[serve.routes]` pattern, as `[name, value]`. */
  readonly params: ReadonlyArray<readonly [string, string]>;
  /** Request headers (lowercased names, arrival order), as `[name, value]`. */
  readonly headers: ReadonlyArray<readonly [string, string]>;
  readonly remoteAddr: string;
  /** The negotiated WebSocket subprotocol, or `null` (always `null` for SSE). */
  readonly subprotocol: string | null;
}

/** The HTTP context of a per-connection WebSocket/SSE handler — the request that opened
 *  the connection, fixed for its life. Read it in your handler via {@link Socket.info} /
 *  {@link SseStream.info}. The {@link ConnectionInfoData} fields plus `param`/`header`. */
export interface ConnectionInfo extends ConnectionInfoData {
  /** One captured route param by name (`:plan` → `param("plan")`), or `undefined`. */
  param(name: string): string | undefined;
  /** The first value of header `name` (case-insensitive), or `undefined`. */
  header(name: string): string | undefined;
}

/** The result of a typed call: `await` it for the reply, or `for await` it to
 *  stream a generator handler's chunks. Function arguments become callbacks that
 *  stay in the caller — the service's invocations travel back as messages.
 *
 *  A method streams only when its handler is an **async generator** (what the runtime
 *  streams); a method returning a value — including an array or other `Iterable`, which
 *  is replied whole — is an ordinary `await` call. */
export type RusmCall<R> =
  R extends AsyncIterable<infer T> ? AsyncIterable<T> & PromiseLike<void> : Promise<Awaited<R>>;

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
  /** This connection's request context — method, path, query, route params, headers,
   *  peer address, and negotiated subprotocol (e.g. `socket.info.param("room")`). */
  readonly info: ConnectionInfo;
  /** Send one **binary** frame back to this connection. */
  send(data: string | Uint8Array): void;
  /** Send one **text** frame back to this connection (the default `send` is binary).
   *  Returns `false` if the socket has closed. */
  sendText(text: string): boolean;
}

/** Wrap the runtime's raw connection context with `param`/`header` lookups (the empty
 *  context for a non-connection process, so the accessors never throw). */
const connectionInfo = (): ConnectionInfo => {
  const raw = Process.connection?.() ?? null;
  const params = raw?.params ?? [];
  const headers = raw?.headers ?? [];
  return {
    method: raw?.method ?? "",
    path: raw?.path ?? "",
    query: raw?.query ?? "",
    params,
    headers,
    remoteAddr: raw?.remoteAddr ?? "",
    subprotocol: raw?.subprotocol ?? null,
    param: (name) => params.find(([k]) => k === name)?.[1],
    header: (name) =>
      headers.find(([k]) => k.toLowerCase() === name.toLowerCase())?.[1],
  };
};

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
  // One runner instance per connection, so the context is fixed — fetch it once.
  let info: ConnectionInfo | undefined;
  const socket = (id: bigint): Socket => ({
    id,
    info: (info ??= connectionInfo()),
    send: (data) => Process.send(id, data),
    sendText: (text) => Process.sendText(text),
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

/** One live SSE stream — the SSE twin of {@link Socket}. Emit events with
 *  {@link SseStream.data}; `id` is its writer pid. SSE is one-way (server → client), so
 *  there are no inbound frames — events reach a handler through its mailbox (typically a
 *  process-group tag it subscribes to in {@link SseHandlers.open}). (Named `SseStream` to
 *  avoid clashing with the byte-stream {@link Stream} from `Process.openStream`.) */
export interface SseStream {
  readonly id: bigint;
  /** This stream's request context — method, path, query, route params, headers, and
   *  peer address (e.g. `stream.info.param("plan")` or `stream.info.header("last-event-id")`). */
  readonly info: ConnectionInfo;
  /** Emit one event to the client (a string is sent as UTF-8). The platform frames it
   *  as a `data:` SSE event. */
  data(payload: string | Uint8Array): void;
  /** End the stream and this process (a server-initiated close). {@link SseHandlers.close}
   *  then fires once, the same teardown as a client disconnect. */
  close(): void;
}

/** Per-connection SSE handlers — the SSE twin of {@link WebSocketHandlers}. */
export interface SseHandlers {
  /** The stream opened. Subscribe to your event source here (e.g.
   *  {@link ProcessApi.registerTag}). */
  open?(stream: SseStream): void;
  /** One event pushed to this stream (e.g. a published message from a subscribed tag).
   *  Emit it with `stream.data(…)`. */
  message(stream: SseStream, event: Uint8Array): void;
  /** The stream closed — the client disconnected, or the handler called
   *  {@link SseStream.close}. */
  close?(stream: SseStream): void;
}

/** Build an SSE component from per-connection handlers — the twin of {@link websocket}.
 *  The host runs one process per connection; you emit events with `stream.data(…)`.
 *  Export the result as the component's default:
 *
 *  ```ts
 *  export default sse({
 *    open: (s) => Process.registerTag("todos"),     // subscribe
 *    message: (s, ev) => s.data(ev),                 // a published event → emit
 *  });
 *  ```
 */
export const sse = (handlers: SseHandlers) => {
  let done = false;
  // One runner instance per connection, so the context is fixed — fetch it once.
  let info: ConnectionInfo | undefined;
  const stream = (id: bigint): SseStream => ({
    id,
    info: (info ??= connectionInfo()),
    data: (payload) => Process.send(id, payload),
    close: () => {
      done = true;
    },
  });
  return {
    sse: {
      open: (conn: bigint) => handlers.open?.(stream(conn)),
      message: (conn: bigint, event: Uint8Array) =>
        handlers.message(stream(conn), event),
      close: (conn: bigint) => handlers.close?.(stream(conn)),
      // The driver checks this after `open` and each `message` to honor a self-close.
      done: () => done,
    },
  };
};
