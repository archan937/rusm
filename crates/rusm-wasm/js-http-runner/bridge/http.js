// http.js — the fetch-handler bridge for the js-http-runner. Loaded after the
// shared webapi.js: it upgrades the stub `Response` to a real one, gives `Request`
// text()/json(), installs a real pull-based `ReadableStream` (webapi's is a one-shot
// byte wrapper), and exposes `__rusm_http` / `__rusm_http_pull` for the Rust host.
//
// Static responses come back as bytes in one call; a `ReadableStream` body is left
// armed and the host pulls it chunk-by-chunk via `__rusm_http_pull` — true streaming
// (SSE), since the host writes each chunk as it's produced.
//
// Scope, by design: a per-request `fetch` handler gets `fetch` + the **no-mailbox** actor
// subset (`actor.js` is shared verbatim; its presence-guards expose only `Process.{self,
// send,whereis,whereisTag}` here — enough to persist and *publish* to subscribers, e.g.
// notify open SSE streams). It deliberately does NOT load `rpc.js` (the typed-client layer:
// `spawn`/`connect`/services), unlike the full js-runner. That's not an omission to fill: a
// `connect` client awaits its reply on a **mailbox**, and a request-scoped handler has none
// (`receive`/`spawn`/`monitor` are absent by construction). Request→reply RPC to a resident
// belongs to the actor-world serving path (a `#[handlers]`/`spawn_js` component); fire-and-
// forget publish via `send`/`whereisTag` is already here. Not a gap.
(() => {
  const G = globalThis;

  G.Response = class Response {
    constructor(body, init) {
      this.body = body ?? null;
      this.status = (init && init.status) || 200;
      this.statusText = (init && init.statusText) || "";
      this.headers = new G.Headers(init && init.headers);
      if (typeof this.body === "string" && !this.headers.has("content-type")) {
        this.headers.set("content-type", "text/plain;charset=UTF-8");
      }
    }
  };

  // Augment the base Request (which already stores url/method/headers/body) with
  // text()/json(). No `body` accessor — the base assigns `this.body`, and a
  // getter-only override would make that assignment throw ("no setter").
  const RequestBase = G.Request;
  G.Request = class Request extends RequestBase {
    async text() {
      if (this.body == null) return "";
      return typeof this.body === "string"
        ? this.body
        : new G.TextDecoder().decode(this.body);
    }
    async json() { return JSON.parse(await this.text()); }
  };

  // A real ReadableStream: supports both `new ReadableStream(uint8Array)` (legacy
  // one-shot) and `new ReadableStream({ start, pull, cancel })` with a controller
  // (enqueue/close/error) — the shape an SSE handler uses.
  G.ReadableStream = class ReadableStream {
    constructor(source) {
      if (source instanceof Uint8Array) {
        this._fixed = source;
        return;
      }
      this._source = source || {};
      this._queue = [];
      this._closed = false;
      this._error = null;
      this._started = false;
      const self = this;
      this._controller = {
        enqueue(chunk) { self._queue.push(chunk); },
        close() { self._closed = true; },
        error(e) { self._error = e; self._closed = true; },
      };
    }
    getReader() {
      const self = this;
      if (self._fixed !== undefined) {
        let done = false;
        return {
          read() {
            if (done) return Promise.resolve({ done: true });
            done = true;
            return Promise.resolve({ done: false, value: self._fixed });
          },
          cancel() { done = true; },
          releaseLock() {},
        };
      }
      return {
        async read() {
          if (!self._started) {
            self._started = true;
            if (self._source.start) await self._source.start(self._controller);
          }
          // Pull until a chunk is queued or the stream closes. A pull that enqueues
          // nothing and doesn't close would spin — real SSE sources enqueue or close
          // each pull, so a bounded guard keeps a buggy guest from hanging the host.
          let spins = 0;
          while (self._queue.length === 0 && !self._closed) {
            if (!self._source.pull) break;
            await self._source.pull(self._controller);
            if (++spins > 1_000_000) break;
          }
          if (self._error) throw self._error;
          if (self._queue.length) return { done: false, value: self._queue.shift() };
          return { done: true };
        },
        cancel(reason) {
          self._closed = true;
          return self._source.cancel ? self._source.cancel(reason) : undefined;
        },
        releaseLock() {},
      };
    }
  };

  const resolveHandler = () => {
    const h = (G.module && G.module.exports) || {};
    if (h.default && typeof h.default.fetch === "function") return h.default.fetch.bind(h.default);
    if (typeof h.default === "function") return h.default;
    if (typeof h.fetch === "function") return h.fetch;
    return null;
  };

  const toBytes = (value) =>
    value instanceof Uint8Array ? value : new G.TextEncoder().encode(String(value));

  // The reader for an in-flight streaming response (one request per instance).
  let activeReader = null;

  G.__rusm_http = async function (method, url, headerPairs, bodyBytes) {
    activeReader = null;
    const handler = resolveHandler();
    if (!handler) {
      throw new Error(
        "component has no fetch handler (export default { fetch } or export default (req) => Response)",
      );
    }
    const req = new G.Request(url, { method, headers: headerPairs, body: bodyBytes });
    const res = await handler(req);
    if (!res) throw new Error("fetch handler returned no Response");

    const headers = [];
    if (res.headers && typeof res.headers.forEach === "function") {
      res.headers.forEach((v, k) => headers.push([String(k), String(v)]));
    }
    const status = res.status || 200;
    const body = res.body;

    // A ReadableStream body streams (the host pulls); anything else is static bytes.
    if (body && typeof body.getReader === "function") {
      activeReader = body.getReader();
      return { status, headers, streaming: true };
    }
    let bytes;
    if (body == null) bytes = new Uint8Array();
    else if (typeof body === "string") bytes = new G.TextEncoder().encode(body);
    else if (body instanceof Uint8Array) bytes = body;
    else bytes = new G.TextEncoder().encode(String(body));
    return { status, headers, streaming: false, body: bytes };
  };

  // Pull the next chunk of a streaming body; null at end-of-stream.
  G.__rusm_http_pull = async function () {
    if (!activeReader) return null;
    const { done, value } = await activeReader.read();
    if (done) {
      activeReader = null;
      return null;
    }
    return toBytes(value);
  };
})();
