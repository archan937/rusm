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

globalThis.Process = {
  self() { return BigInt(__own_pid()); },
  list() { return __list().map(BigInt); },
  // Spawn a registered component by name (capability-gated); returns its pid. With a
  // runtime `source` (`inline:<js>` / `kv:<bucket>/<key>` / `url:`/`http(s)://…`), spawns
  // a dynamic JS instance of a runner template, running the loaded JS under the template's
  // declared profile.
  spawn(name, source) {
    return source === undefined
      ? BigInt(__spawn(name))
      : BigInt(__spawnFrom(name, source));
  },
  // Monitor a process: its death arrives as a `{ __down, reason }` message.
  monitor(pid) { __monitor(String(pid)); },
  send(to, msg) {
    if (typeof msg === "string") __send_text(String(to), msg);
    else __send(String(to), msg);
  },
  // Resolves to the next message as a Uint8Array. With `timeoutMs`, it's Erlang's
  // `receive … after`: resolves to null if the deadline passes first. Set-aside RPC
  // mail is delivered immediately (a pending message can't time out).
  receive(timeoutMs) {
    if (__inbox.length) return Promise.resolve(__inbox.shift());
    if (timeoutMs === undefined) return Promise.resolve(__receive());
    const m = __receive_timeout(timeoutMs);
    return Promise.resolve(m === undefined ? null : m);
  },
  // Resolves to the next message decoded as UTF-8 (null on `timeoutMs` timeout).
  receiveText(timeoutMs) {
    if (__inbox.length) return Promise.resolve(new TextDecoder().decode(__inbox.shift()));
    if (timeoutMs === undefined) return Promise.resolve(__receive_text());
    const m = __receive_timeout(timeoutMs);
    return Promise.resolve(m === undefined ? null : new TextDecoder().decode(m));
  },
  register(name) { return __register(name); },
  whereis(name) { const p = __whereis(name); return p === "" ? null : BigInt(p); },
  isAlive(pid) { return __is_alive(String(pid)); },
  kill(pid) { return __kill(String(pid)); },
  setLabel(label) { __set_label(label); },
  // Process-group tags (Erlang `pg`): tag this process, leave a tag, list a group's live
  // members (pids), or terminate a whole group (count). killTag needs process-control.
  registerTag(tag) { __register_tag(tag); },
  unregisterTag(tag) { __unregister_tag(tag); },
  whereisTag(tag) { return __whereis_tag(tag).map((p) => BigInt(p)); },
  killTag(tag) { return __kill_tag(tag); },
  openStream(to) { const h = __stream_open(String(to)); return h < 0 ? null : new Stream(h); },
  acceptStream() { return new Stream(__stream_accept()); },
};

globalThis.__rusm_Stream = Stream;
