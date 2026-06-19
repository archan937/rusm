// process.js — the RUSM actor API for JS guests.
//
// Separation of concerns: this file is *only* the `Process`/`Stream` bridge over
// the host primitives the runner installs (the `__*` globals). Web API polyfills
// live in webapi.js; the RPC/service layer (typed clients, dispatch) in rpc.js;
// lifecycle/host wiring in the runner (lib.rs).
//
// Async by design: `receive`/`receiveText` and `Stream.read` return Promises, so
// guests `await` them — idiomatic JS, and they compose with other Promises. The
// host call still suspends the whole instance's fiber (freeing the Tokio worker),
// so "blocking" is cheap; the Promise is driven by the QuickJS job queue.
//
// Pids cross the boundary as decimal strings (a u64 doesn't fit a JS number) and
// surface as BigInt; messages/chunks are Uint8Array, with text helpers.

// Messages the RPC client set aside while awaiting a reply (so a typed call never
// swallows the app's own mail). `Process.receive*` drains this before the host.
const __inbox = [];
globalThis.__rusm_stash = (raw) => __inbox.push(raw);

class Stream {
  constructor(handle) { this.handle = handle; }
  // write accepts a string (UTF-8) or a Uint8Array.
  write(chunk) {
    return typeof chunk === "string"
      ? __stream_write_text(this.handle, chunk)
      : __stream_write(this.handle, chunk);
  }
  close() { __stream_close(this.handle); }
  // Resolves to a Uint8Array, or null at end-of-stream (host None → undefined → null).
  read() {
    const c = __stream_read(this.handle);
    return Promise.resolve(c === undefined ? null : c);
  }
}

// Each method is installed only when the host primitive backing it is present, so a
// runner that wires a *subset* of the actor ABI exposes exactly the ops it can honor —
// never a method that would fail when called. The actor js-runner wires the full set, so
// it gets the full `Process` unchanged; the request-only js-http-runner wires only the
// no-mailbox ops (self/send/whereis/whereisTag), so it has no receive/spawn/monitor/tag-
// join (a per-request handler has no mailbox to back them).
const has = (fn) => typeof globalThis[fn] === "function";
const P = {};
if (has("__own_pid")) P.self = () => BigInt(__own_pid());
// The per-connection serving context (a ws/sse handler's request), or null otherwise.
// The SDK's websocket()/sse() expose it as `socket.info()` / `stream.info()`.
if (has("__connection")) P.connection = () => JSON.parse(__connection() ?? "null");
// Send a text WebSocket frame on this connection (binary uses Process.send to the writer).
if (has("__ws_send_text")) P.sendText = (text) => __ws_send_text(text);
// Close this WebSocket connection with a status code + reason (a server-initiated close).
if (has("__ws_close")) P.wsClose = (code, reason) => __ws_close(code, reason ?? "");
if (has("__list")) P.list = () => __list().map(BigInt);
// Spawn a registered component by name (capability-gated); returns its pid. With a
// runtime `source` (`inline:<js>` / `kv:<bucket>/<key>` / `url:`/`http(s)://…`), spawns a
// dynamic JS instance of a runner template under the template's declared profile.
if (has("__spawn"))
  P.spawn = (name, source) =>
    source === undefined ? BigInt(__spawn(name)) : BigInt(__spawnFrom(name, source));
// Monitor a process: its death arrives as a `{ __down, reason }` message.
if (has("__monitor")) P.monitor = (pid) => __monitor(String(pid));
if (has("__send"))
  P.send = (to, msg) => {
    if (typeof msg === "string") __send_text(String(to), msg);
    else __send(String(to), msg);
  };
if (has("__receive")) {
  // Resolves to the next message as a Uint8Array. With `timeoutMs`, it's Erlang's
  // `receive … after`: resolves to null if the deadline passes first. Set-aside RPC mail
  // is delivered immediately (a pending message can't time out).
  P.receive = (timeoutMs) => {
    if (__inbox.length) return Promise.resolve(__inbox.shift());
    if (timeoutMs === undefined) return Promise.resolve(__receive());
    const m = __receive_timeout(timeoutMs);
    return Promise.resolve(m === undefined ? null : m);
  };
  // Resolves to the next message decoded as UTF-8 (null on `timeoutMs` timeout).
  P.receiveText = (timeoutMs) => {
    if (__inbox.length) return Promise.resolve(new TextDecoder().decode(__inbox.shift()));
    if (timeoutMs === undefined) return Promise.resolve(__receive_text());
    const m = __receive_timeout(timeoutMs);
    return Promise.resolve(m === undefined ? null : new TextDecoder().decode(m));
  };
}
if (has("__register")) P.register = (name) => __register(name);
if (has("__whereis"))
  P.whereis = (name) => {
    const p = __whereis(name);
    return p === "" ? null : BigInt(p);
  };
if (has("__is_alive")) P.isAlive = (pid) => __is_alive(String(pid));
if (has("__kill")) P.kill = (pid) => __kill(String(pid));
if (has("__set_label")) P.setLabel = (label) => __set_label(label);
// Process-group tags (Erlang `pg`): tag this process, leave a tag, list a group's live
// members (pids), or terminate a whole group (count). killTag needs process-control.
if (has("__register_tag")) P.registerTag = (tag) => __register_tag(tag);
if (has("__unregister_tag")) P.unregisterTag = (tag) => __unregister_tag(tag);
if (has("__whereis_tag")) P.whereisTag = (tag) => __whereis_tag(tag).map((p) => BigInt(p));
if (has("__kill_tag")) P.killTag = (tag) => __kill_tag(tag);
if (has("__stream_open"))
  P.openStream = (to) => {
    const h = __stream_open(String(to));
    return h < 0 ? null : new Stream(h);
  };
if (has("__stream_accept")) P.acceptStream = () => new Stream(__stream_accept());
globalThis.Process = P;

globalThis.__rusm_Stream = Stream;
