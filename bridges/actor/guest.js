// Canonical source: bridges/actor/guest.js — the actor bridge's JS guest binding (the
// `Process` API). Synced into the js-runner (crates/rusm-wasm/js-runner/bridge/actor.js,
// which the js-http-runner also include_str!s) by `make sync-bridges`; edit this file, not
// the copy. `bridge_guest_in_sync` guards drift.
//
// Separation of concerns: this file is *only* the `Process` actor API over the host
// primitives the runner installs (the `__*` globals). Web API polyfills live in webapi.js;
// `console` in log.js; the cross-process `Stream` in stream.js; tags in pg.js; the
// connection controls in serve.js; the RPC/service layer in rpc.js.
//
// Async by design: `receive`/`receiveText` return Promises, so guests `await` them —
// idiomatic JS. The host call still suspends the whole instance's fiber (freeing the Tokio
// worker), so "blocking" is cheap; the Promise is driven by the QuickJS job queue.
//
// Pids cross the boundary as decimal strings (a u64 doesn't fit a JS number) and surface as
// BigInt; messages/chunks are Uint8Array, with text helpers.

// Messages the RPC client set aside while awaiting a reply (so a typed call never
// swallows the app's own mail). `Process.receive*` drains this before the host. The client
// holds set-aside mail in a LOCAL buffer during the call and restores it to the FRONT here
// afterward — never re-inserting mid-call, since `receive` drains __inbox first and would
// otherwise re-read the same message forever (a spin/hang).
const __inbox = [];
globalThis.__rusm_unstash_front = (saved) => __inbox.unshift(...saved);

// `Stream` (the cross-process byte stream) is the stream bridge's binding — defined as a
// global in bridge/stream.js (eval'd before this); Process.openStream/acceptStream below
// construct it.

// Each method is installed only when the host primitive backing it is present, so a
// runner that wires a *subset* of the actor ABI exposes exactly the ops it can honor —
// never a method that would fail when called. The actor js-runner wires the full set, so
// it gets the full `Process` unchanged; the request-only js-http-runner wires only the
// no-mailbox ops (self/send/whereis/whereisTag), so it has no receive/spawn/monitor/tag-
// join (a per-request handler has no mailbox to back them).
const has = (fn) => typeof globalThis[fn] === "function";
const P = {};
if (has("__own_pid")) P.self = () => BigInt(__own_pid());
// The per-connection serving controls (`Process.connection`/`sendText`/`wsClose`/`sseSend`)
// are the serve bridge — see bridge/serve.js, eval'd after this; it augments Process.
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
// Schedule a delayed message (Erlang's erlang:send_after/3). Returns a timer handle
// (a number) that Process.cancelTimer can abort before it fires. `msg` may be a
// Uint8Array (binary) or a string (UTF-8 encoded). A fired or unknown handle is a
// silent no-op on the delivery side.
if (has("__send_after"))
  P.sendAfter = (to, delayMs, msg) => {
    const bytes = typeof msg === "string" ? new TextEncoder().encode(msg) : msg;
    return __send_after(String(to), delayMs, bytes);
  };
// Cancel a pending timer by the handle returned by sendAfter. Returns true if the
// timer was found and aborted; false if unknown (already fired or never issued).
if (has("__cancel_timer")) P.cancelTimer = (timerRef) => __cancel_timer(timerRef);
// Process-group tags (`Process.registerTag`/etc.) are the pg bridge — see bridge/pg.js,
// eval'd after this; it augments Process with the tag ops.
if (has("__stream_open"))
  P.openStream = (to) => {
    const h = __stream_open(String(to));
    return h < 0 ? null : new Stream(h);
  };
if (has("__stream_accept")) P.acceptStream = () => new Stream(__stream_accept());
globalThis.Process = P;
