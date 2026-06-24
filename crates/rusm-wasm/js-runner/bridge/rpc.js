// rpc.js — the RPC / service layer over the raw Process actor API (process.js).
//
// Two halves of one protocol:
//   • service — a component that EXPORTS functions. The runner drives __rusm_entry,
//     which dispatches each request to the matching export and replies.
//   • client  — `spawn(name)` returns a typed proxy. `proxy.method(...)` is a call
//     you `await` for the reply, or `for await` to stream a generator handler's
//     chunks; `proxy.cast.method(...)` is fire-and-forget; `proxy.pid`/`.stop()`
//     manage the process. Function arguments become **callbacks** routed home.
//
// Wire protocol (JSON over the byte mailbox):
//   request   { op, args, from, ref }            — ref omitted ⇒ a cast
//   request   { op, args, from, ref, stream:true }— streaming: reply rides a stream
//   reply     { ref, ok } | { ref, err }
//   callback  { op:"__cb", cbref, args }          — invoke a caller-side function
// Streamed chunks are JSON-encoded values, one per stream write.

const __td = new TextDecoder();
let __ref = 0;
let __cbSeq = 0;
// Caller-side functions passed as args, by id, so a service's callback message can
// be routed back to the live closure (the function never leaves this instance).
const __callbacks = {};

// A JSON.stringify replacer that encodes a function argument as a {__cb:id} marker
// (registering it in `cbs`) in a single serialization pass — no pre-clone of args.
function __cbReplacer(cbs) {
  return (_key, value) => {
    if (typeof value === "function") {
      const id = ++__cbSeq;
      cbs[id] = value;
      return { __cb: id };
    }
    return value;
  };
}

// Encode + send a request, registering any callback args so their invocations can
// be routed home. Returns the ids registered (to clean up when the call is done).
function __sendRequest(pid, msg) {
  const cbs = {};
  const payload = JSON.stringify(msg, __cbReplacer(cbs));
  const ids = Object.keys(cbs);
  for (const id of ids) __callbacks[id] = cbs[id];
  Process.send(pid, payload);
  return ids;
}

// Service side: replace {__cb:id} markers with functions that message the caller.
// Only called when the request actually carries a callback (see __rusm_serve).
function __decodeArgs(args, from) {
  const dec = (v) => {
    if (Array.isArray(v)) return v.map(dec);
    if (v && typeof v === "object") {
      if (typeof v.__cb === "number" && Object.keys(v).length === 1) {
        const cbref = v.__cb;
        return (...a) =>
          Process.send(BigInt(from), JSON.stringify({ op: "__cb", cbref, args: a }));
      }
      const o = {};
      for (const k of Object.keys(v)) o[k] = dec(v[k]);
      return o;
    }
    return v;
  };
  return (args || []).map(dec);
}

// A call: send the request, then await the matching reply — servicing callback
// messages and stashing unrelated mail so the app's own receive still sees it.
async function __call(pid, op, args, expectReply) {
  const ref = expectReply ? ++__ref : undefined;
  const msg = { op, args, from: Process.self().toString() };
  if (expectReply) msg.ref = ref;
  const ids = __sendRequest(pid, msg);
  if (!expectReply) return undefined; // cast: fire-and-forget
  // Hold non-matching mail in a LOCAL buffer for the duration of the call, restored to the
  // inbox front in `finally` — never re-stashing mid-loop, since `receive` drains the inbox
  // first and a re-stashed message would be re-read forever while the reply waits behind it.
  const setAside = [];
  try {
    for (;;) {
      const raw = await Process.receive();
      let m;
      try { m = JSON.parse(__td.decode(raw)); } catch { setAside.push(raw); continue; }
      if (m && m.op === "__cb") { __callbacks[m.cbref]?.(...(m.args || [])); continue; }
      // A reply carries `ok`/`err`; match by ref AND shape so a concurrent inbound request
      // whose per-process `ref` collides with ours isn't mis-read as the reply.
      const isReply = m && typeof m === "object" && ("ok" in m || "err" in m);
      if (isReply && m.ref === ref) {
        if ("err" in m) throw new Error(m.err);
        return m.ok;
      }
      setAside.push(raw); // not our reply — hold locally for the app
    }
  } finally {
    for (const id of ids) delete __callbacks[id];
    if (setAside.length) __rusm_unstash_front(setAside);
  }
}

// A call with a deadline: fails with Error("timeout") if no matching reply arrives within
// timeoutMs milliseconds. Uses Process.receive(remaining) on each iteration so the host
// fiber parks for at most `remaining` ms rather than forever. Tracks the deadline via
// Date.now() so the budget spans the entire call, not just one receive.
async function __callTimeout(pid, op, args, timeoutMs) {
  const ref = ++__ref;
  const msg = { op, args, from: Process.self().toString(), ref };
  const ids = __sendRequest(pid, msg);
  const deadline = Date.now() + timeoutMs;
  const setAside = [];
  try {
    for (;;) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("timeout");
      const raw = await Process.receive(remaining);
      if (raw === null) throw new Error("timeout");
      let m;
      try { m = JSON.parse(__td.decode(raw)); } catch { setAside.push(raw); continue; }
      if (m && m.op === "__cb") { __callbacks[m.cbref]?.(...(m.args || [])); continue; }
      const isReply = m && typeof m === "object" && ("ok" in m || "err" in m);
      if (isReply && m.ref === ref) {
        if ("err" in m) throw new Error(m.err);
        return m.ok;
      }
      setAside.push(raw);
    }
  } finally {
    for (const id of ids) delete __callbacks[id];
    if (setAside.length) __rusm_unstash_front(setAside);
  }
}

// A streaming call: the service opens a byte stream back; yield each JSON chunk.
async function* __streamCall(pid, op, args) {
  const ref = ++__ref;
  __sendRequest(pid, { op, args, from: Process.self().toString(), ref, stream: true });
  const s = Process.acceptStream();
  let chunk;
  while ((chunk = await s.read()) !== null) {
    yield JSON.parse(__td.decode(chunk));
  }
}

// A method result: awaitable (→ a call) and async-iterable (→ a streaming call).
// The caller chooses by `await proxy.m(...)` vs `for await (... of proxy.m(...))`.
// `defaultTimeout` (set by withTimeout) routes calls through __callTimeout instead.
function __invoke(pid, op, args, defaultTimeout) {
  let p;
  const call = () => (p = p || (defaultTimeout !== undefined
    ? __callTimeout(pid, op, args, defaultTimeout)
    : __call(pid, op, args, true)));
  return {
    then: (res, rej) => call().then(res, rej),
    catch: (f) => call().catch(f),
    finally: (f) => call().finally(f),
    [Symbol.asyncIterator]: () => __streamCall(pid, op, args)[Symbol.asyncIterator](),
  };
}

function __castClient(pid) {
  return new Proxy({}, { get: (_t, op) => (...args) => __call(pid, String(op), args, false) });
}

// A typed client over a process `pid`: each property is a call (`await`) / stream
// (`for await`); `.cast` is fire-and-forget; `.pid` is the pid; `.withTimeout(ms)` returns
// a variant where every call has a deadline. `owned` (a spawned process) also exposes
// `.stop()` — a connected client to a resident you don't own does not.
function __clientProxy(pid, owned, defaultTimeout) {
  return new Proxy(
    {},
    {
      get(_t, op) {
        if (op === "pid") return pid;
        if (op === "cast") return __castClient(pid);
        if (op === "stop") return owned ? () => Process.kill(pid) : undefined;
        if (op === "withTimeout") return (ms) => __clientProxy(pid, owned, ms);
        return (...args) => __invoke(pid, String(op), args, defaultTimeout);
      },
    },
  );
}

// `spawn(name)` → a typed client over a freshly spawned component.
globalThis.spawn = function (name) {
  return __clientProxy(Process.spawn(name), true);
};

// `connect(name | pid)` → a typed client over an EXISTING process (a resident): a registered
// name is resolved with `whereis`, or a `bigint` pid is used directly. Unlike `spawn` it
// starts nothing — the TS twin of Rust's `Client::connect(pid)` / Go's `Call(pid, …)`.
globalThis.connect = function (target) {
  let pid = target;
  if (typeof target !== "bigint") {
    pid = Process.whereis(target);
    if (pid == null) throw new Error("connect: no process registered as " + target);
  }
  return __clientProxy(pid, false);
};

// `callTimeout(pid, op, timeoutMs, ...args)` — a direct call with a deadline, without a
// typed proxy. Useful when the caller has a pid but not a typed client.
globalThis.callTimeout = function (pid, op, timeoutMs, ...args) {
  return __callTimeout(pid, op, args, timeoutMs);
};

// Service dispatch: receive a request, call the matching exported handler, and
// either reply with its value or stream a generator handler's chunks back.
async function __rusm_serve(handlers) {
  for (;;) {
    let text;
    try { text = await Process.receiveText(); } catch { continue; }
    let req;
    try { req = JSON.parse(text); } catch { continue; }
    const { op, args, from, ref, stream } = req || {};
    const fn = handlers[op];
    // Only walk the args to rebuild callbacks when one is actually present.
    const decoded = text.includes('"__cb"') ? __decodeArgs(args, from) : args || [];
    if (typeof fn !== "function") {
      if (ref != null && from != null) {
        Process.send(BigInt(from), JSON.stringify({ ref, err: "no such function: " + op }));
      }
      continue;
    }
    if (stream) {
      // Streaming handler (a generator / async-iterable): pump chunks down a stream.
      const out = Process.openStream(from);
      if (out) {
        try {
          for await (const chunk of await fn(...decoded)) out.write(JSON.stringify(chunk));
        } catch (_e) {
          // a handler error just ends the stream early (close below)
        }
        out.close();
      }
      continue;
    }
    let reply;
    try { reply = { ref, ok: await fn(...decoded) }; }
    catch (e) { reply = { ref, err: String((e && e.message) || e) }; }
    if (ref != null && from != null) Process.send(BigInt(from), JSON.stringify(reply));
  }
}

// A guest **supervisor**: spawn + monitor named children and restart per a
// strategy ("one_for_one" | "one_for_all" | "rest_for_one") when one dies. A dead
// child arrives as a `{ __down }` message (no polling). `await supervise({...})`.
// A thin facade over the host's single native supervisor (the `supervise` ABI,
// wired as `__supervise`) — the same one the Rust SDK uses, so there's one restart
// implementation. The host links the supervisor to us; we park as its owner until
// the link takes us (on give-up) or we're killed (which tears the children down).
globalThis.supervise = async function ({ strategy = "one_for_one", children = [], maxRestarts = 0, maxSeconds = 0 }) {
  __supervise(strategy, children, maxRestarts >>> 0, (maxSeconds * 1000) >>> 0);
  for (;;) {
    try { await Process.receive(); } catch { return; }
  }
};

// Called by the runner after evaluating the bundle; returns the promise the runner
// drives to completion. A component is a SERVICE if it exports named functions (run
// the dispatch loop), a WORKER if it exports `default` (run it), or a bare script
// (already ran during eval — nothing left to drive).
globalThis.__rusm_entry = function () {
  const h = (globalThis.module && globalThis.module.exports) || {};
  // A resident HTTP server (the host set the role): drive `export default { fetch }`
  // (or a default handler function) as a stateful serving loop — module state
  // persists across requests because the instance is long-lived.
  if (globalThis.__rusm_role === "http") return __rusm_http_serve(h.default);
  // A per-connection handler carrying an `open/message/close` lifecycle table: a
  // WebSocket (`export default websocket({…})` → `.websocket`) or an SSE stream
  // (`export default sse({…})` → `.sse`). The host runs one sandboxed process per
  // connection; one driver runs both (the table differs, the lifecycle doesn't).
  if (h.default && h.default.websocket) return __rusm_connection(h.default.websocket);
  if (h.default && h.default.sse) return __rusm_connection(h.default.sse);
  const named = Object.keys(h).filter((k) => k !== "default" && typeof h[k] === "function");
  if (named.length) {
    const handlers = {};
    for (const k of named) handlers[k] = h[k];
    return __rusm_serve(handlers);
  }
  if (typeof h.default === "function") return h.default();
  return Promise.resolve();
};

// Resident HTTP serving loop: each `{op:"fetch", ref, from, args:[req]}` envelope
// from the host gateway becomes a `Request`, dispatched to the guest's handler; the
// `Response` is encoded back as `{ref, ok:{status, headers, body}}`. One instance
// serves every request, so the handler's closure state lives across them.
async function __rusm_http_serve(handler) {
  const fetch = typeof handler === "function" ? handler : handler && handler.fetch;
  if (typeof fetch !== "function") return; // not an HTTP handler — nothing to serve
  for (;;) {
    let text;
    try { text = await Process.receiveText(); } catch { continue; }
    let req;
    try { req = JSON.parse(text); } catch { continue; }
    if (req.op !== "fetch" || req.from == null) continue;
    const a = (req.args && req.args[0]) || {};
    const request = new Request(a.url || "/", {
      method: a.method || "GET",
      headers: a.headers || [],
      body: a.body ? __b64decode(a.body) : null, // body crosses as base64
    });
    let response;
    try {
      response = await fetch(request);
    } catch (e) {
      if (req.ref != null) {
        Process.send(BigInt(req.from), JSON.stringify({ ref: req.ref, err: String((e && e.message) || e) }));
      }
      continue;
    }
    // A ReadableStream body streams (SSE): send the head, then write each chunk down
    // a byte stream to the responder. A buffered body replies in one message.
    const body = response && response.body;
    if (body && typeof body.getReader === "function") {
      await __http_stream_response(req, response, body);
    } else if (req.ref != null) {
      Process.send(BigInt(req.from), JSON.stringify({ ref: req.ref, ok: __http_response_to_wire(response) }));
    }
  }
}

// Stream a ReadableStream response body to the responder: a head with `stream:true`,
// then each chunk on a byte stream (true SSE; the host flushes each as a frame).
async function __http_stream_response(req, response, body) {
  if (req.ref == null) return;
  const headers = [];
  const h = response.headers;
  if (h && typeof h.entries === "function") {
    for (const [k, v] of h.entries()) headers.push([k, v]);
  }
  Process.send(
    BigInt(req.from),
    JSON.stringify({ ref: req.ref, ok: { status: response.status || 200, headers, stream: true } }),
  );
  const out = Process.openStream(req.from);
  if (!out) return;
  const enc = new TextEncoder();
  const reader = body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    out.write(value instanceof Uint8Array ? value : enc.encode(String(value)));
  }
  out.close();
}

// Parse a monitor `__down` message (`{"__down":"<pid>","reason":...}`) into the dead
// process's pid (a BigInt), or null for an ordinary message. The single source for
// `__down` decoding in this bridge; a fast prefix check never parses ordinary frames.
function __downPid(bytes) {
  const PREFIX = '{"__down":"';
  if (!bytes || bytes.length < PREFIX.length) return null;
  for (let i = 0; i < PREFIX.length; i++) {
    if (bytes[i] !== PREFIX.charCodeAt(i)) return null;
  }
  try {
    const obj = JSON.parse(new TextDecoder().decode(bytes));
    return obj && obj.__down != null ? BigInt(obj.__down) : null;
  } catch {
    return null;
  }
}

// Per-connection lifecycle driver (mirrors the Rust `ws::serve` / `sse::serve` loop),
// shared by WebSocket and SSE: the host runs one process per connection, delivering the
// writer pid first (the reply target), then each later message — an inbound frame (WS) or
// a pushed event (SSE). We `monitor` the writer — its death IS the disconnect, clean or
// dropped — so the guest's `close` fires before this process exits. `conn` passed to the
// callbacks is the writer pid (the SDK wraps it into a `Socket` or `Stream`). An optional
// `table.done()` lets a handler self-stop (SSE `stream.close()`): we end after `close`,
// the process exits, and the host writer ends the body (it monitors this process).
async function __rusm_connection(table) {
  if (!table || typeof table.message !== "function") return; // not a lifecycle handler
  const stopped = () => table.done && table.done();
  const writer = BigInt(await Process.receiveText()); // message 1: the writer pid
  Process.monitor(writer);
  if (table.open) await table.open(writer);
  while (!stopped()) {
    let data;
    try { data = await Process.receive(); } catch { return; }
    const dead = __downPid(data);
    if (dead !== null) {
      if (dead === writer) break; // client disconnected
      continue; // a `__down` for some other monitored pid — not an inbound frame/event
    }
    await table.message(writer, data);
  }
  if (table.close) await table.close(writer);
}

// Encode a guest `Response` to the wire shape the host expects: body as base64
// (compact + binary-safe), headers as pairs.
function __http_response_to_wire(response) {
  const enc = new TextEncoder();
  const b = response && response.body;
  let bytes;
  if (b == null) bytes = new Uint8Array();
  else if (b instanceof Uint8Array) bytes = b;
  else if (typeof b === "string") bytes = enc.encode(b);
  else bytes = enc.encode(String(b));
  const headers = [];
  const h = response && response.headers;
  if (h && typeof h.entries === "function") {
    for (const [k, v] of h.entries()) headers.push([k, v]);
  }
  return { status: (response && response.status) || 200, headers, body: __b64encode(bytes) };
}

// Standard base64 (no external deps) for the request/response body on the wire.
const __B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
function __b64encode(bytes) {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const b0 = bytes[i];
    const b1 = i + 1 < bytes.length ? bytes[i + 1] : 0;
    const b2 = i + 2 < bytes.length ? bytes[i + 2] : 0;
    out += __B64[b0 >> 2] + __B64[((b0 & 3) << 4) | (b1 >> 4)];
    out += i + 1 < bytes.length ? __B64[((b1 & 15) << 2) | (b2 >> 6)] : "=";
    out += i + 2 < bytes.length ? __B64[b2 & 63] : "=";
  }
  return out;
}
function __b64decode(str) {
  const out = [];
  let acc = 0;
  let bits = 0;
  for (const ch of str) {
    const v = __B64.indexOf(ch);
    if (v < 0) continue; // skip padding / whitespace
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 0xff);
    }
  }
  return new Uint8Array(out);
}
