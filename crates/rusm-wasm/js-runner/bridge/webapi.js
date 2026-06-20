// webapi.js — generic Web API polyfills for QuickJS (rquickjs).
//
// Separation of concerns: the base Web standards (`TextEncoder`, `URL`, base64,
// `ReadableStream`, …) — no RUSM/actor knowledge. `console` is the **log bridge**
// (bridge/log.js, eval'd before this); the actor bridge lives in process.js. A TS
// developer never imports or sees this; it's installed by the js-runner before their
// bundle runs, so these globals are simply present.
//
// `fetch` is intentionally a clear-erroring stub: real HTTP needs RUSM to host
// `wasi:http` (roadmap). Everything else here is self-contained.

(() => {
  const G = globalThis;
  const def = (name, value) => {
    if (!G[name]) G[name] = value;
  };

  // (`console` is the `log` bridge's polyfill — see bridge/log.js, eval'd before this —
  // so this file stays pure standards with no `__log`/actor dependency.)

  // base64 (btoa/atob) — the standard Web APIs, operating on binary (Latin-1) strings.
  // Self-contained; the ecosystem (JWTs, data URLs, hashing) expects them present.
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  def("btoa", (input) => {
    const s = String(input);
    for (let i = 0; i < s.length; i++) {
      if (s.charCodeAt(i) > 0xff) {
        throw new Error("btoa: a character is outside the Latin1 range");
      }
    }
    let out = "";
    for (let i = 0; i < s.length; i += 3) {
      const c0 = s.charCodeAt(i);
      const has1 = i + 1 < s.length;
      const has2 = i + 2 < s.length;
      const c1 = has1 ? s.charCodeAt(i + 1) : 0;
      const c2 = has2 ? s.charCodeAt(i + 2) : 0;
      out += B64[c0 >> 2];
      out += B64[((c0 & 0b11) << 4) | (c1 >> 4)];
      out += has1 ? B64[((c1 & 0b1111) << 2) | (c2 >> 6)] : "=";
      out += has2 ? B64[c2 & 0b111111] : "=";
    }
    return out;
  });
  def("atob", (input) => {
    const s = String(input).replace(/[\t\n\f\r ]/g, "");
    if (s.length % 4 === 1) throw new Error("atob: invalid base64 length");
    let out = "";
    let acc = 0;
    let bits = 0;
    for (const ch of s) {
      if (ch === "=") break;
      const v = B64.indexOf(ch);
      if (v === -1) throw new Error("atob: invalid base64 character");
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 0xff);
      }
    }
    return out;
  });

  class TextEncoderPF {
    encode(str) {
      const out = [];
      for (let i = 0; i < str.length; i++) {
        let c = str.charCodeAt(i);
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push((c >> 6) | 0xc0, (c & 0x3f) | 0x80);
        else if (c < 0xd800 || c >= 0xe000)
          out.push((c >> 12) | 0xe0, ((c >> 6) & 0x3f) | 0x80, (c & 0x3f) | 0x80);
        else {
          // A high surrogate (0xD800–0xDBFF) plus the following low surrogate is one
          // astral code point (e.g. an emoji) — encode it as 4 UTF-8 bytes. Without
          // this each surrogate became a bogus 3-byte sequence → mojibake.
          const lo = str.charCodeAt(++i);
          c = 0x10000 + (((c & 0x3ff) << 10) | (lo & 0x3ff));
          out.push(
            (c >> 18) | 0xf0,
            ((c >> 12) & 0x3f) | 0x80,
            ((c >> 6) & 0x3f) | 0x80,
            (c & 0x3f) | 0x80,
          );
        }
      }
      return new Uint8Array(out);
    }
  }
  class TextDecoderPF {
    constructor(_enc = "utf-8") {}
    decode(input) {
      if (!input) return "";
      const b = input instanceof ArrayBuffer ? new Uint8Array(input) : input;
      let s = "";
      for (let i = 0; i < b.length; ) {
        const x = b[i++];
        if (x < 0x80) s += String.fromCharCode(x);
        else if ((x & 0xe0) === 0xc0) s += String.fromCharCode(((x & 0x1f) << 6) | (b[i++] & 0x3f));
        else if ((x & 0xf0) === 0xe0)
          s += String.fromCharCode(((x & 0x0f) << 12) | ((b[i++] & 0x3f) << 6) | (b[i++] & 0x3f));
        else {
          const cp =
            (((x & 0x07) << 18) | ((b[i++] & 0x3f) << 12) | ((b[i++] & 0x3f) << 6) | (b[i++] & 0x3f)) -
            0x10000;
          s += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        }
      }
      return s;
    }
  }

  class HeadersPF {
    constructor(init) {
      this._d = {};
      const add = (k, v) => (this._d[String(k).toLowerCase()] = v);
      if (Array.isArray(init)) init.forEach(([k, v]) => add(k, v));
      else if (init) Object.entries(init).forEach(([k, v]) => add(k, v));
    }
    get(k) { return this._d[k.toLowerCase()] ?? null; }
    set(k, v) { this._d[k.toLowerCase()] = v; }
    has(k) { return k.toLowerCase() in this._d; }
    delete(k) { delete this._d[k.toLowerCase()]; }
    forEach(fn) { Object.entries(this._d).forEach(([k, v]) => fn(v, k)); }
    entries() { return Object.entries(this._d)[Symbol.iterator](); }
  }

  class URLSearchParamsPF {
    constructor(init) {
      this._d = [];
      if (typeof init === "string") {
        (init[0] === "?" ? init.slice(1) : init).split("&").forEach((p) => {
          if (!p) return;
          const [k, v = ""] = p.split("=");
          this._d.push([decodeURIComponent(k), decodeURIComponent(v)]);
        });
      } else if (Array.isArray(init)) this._d = [...init];
      else if (init) Object.entries(init).forEach(([k, v]) => this._d.push([k, v]));
    }
    get(k) { return this._d.find(([key]) => key === k)?.[1] ?? null; }
    getAll(k) { return this._d.filter(([key]) => key === k).map(([, v]) => v); }
    set(k, v) {
      const i = this._d.findIndex(([key]) => key === k);
      if (i >= 0) this._d[i][1] = v; else this._d.push([k, v]);
    }
    append(k, v) { this._d.push([k, v]); }
    has(k) { return this._d.some(([key]) => key === k); }
    delete(k) { this._d = this._d.filter(([key]) => key !== k); }
    forEach(fn) { this._d.forEach(([k, v]) => fn(v, k)); }
    entries() { return this._d[Symbol.iterator](); }
    toString() {
      return this._d.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join("&");
    }
  }

  class URLPF {
    constructor(url, base) {
      let full = String(url);
      if (base && !full.match(/^[a-z][a-z+\-.]*:/i))
        full = String(base).replace(/\/$/, "") + "/" + full.replace(/^\//, "");
      this.href = full;
      const m = full.match(/^(([a-z][a-z+\-.]*):\/\/([^/:?#]*)(?::(\d+))?)(\/[^?#]*)?(\?[^#]*)?(#.*)?$/i);
      if (m) {
        this.protocol = m[2] + ":"; this.hostname = m[3]; this.port = m[4] ?? "";
        this.host = this.hostname + (this.port ? ":" + this.port : ""); this.origin = m[1];
        this.pathname = m[5] ?? "/"; this.search = m[6] ?? ""; this.hash = m[7] ?? "";
      } else {
        this.protocol = this.hostname = this.port = this.host = this.origin = this.search = this.hash = "";
        this.pathname = full;
      }
      this.searchParams = new URLSearchParamsPF(this.search ? this.search.slice(1) : "");
    }
    toString() { return this.href; }
  }

  // Supports both `new ReadableStream(uint8Array)` (legacy one-shot) and
  // `new ReadableStream({ start, pull, cancel })` with a controller — the shape an
  // SSE handler uses. (The per-request js-http-runner installs an identical class;
  // sharing it here lets the resident runner stream too.)
  class ReadableStreamPF {
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
          // Pull until a chunk is queued or the stream closes. A bounded guard keeps
          // a buggy guest (a pull that never enqueues or closes) from hanging.
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
  }

  class BlobPF {
    constructor(parts) { this._s = (parts ?? []).map(String).join(""); }
    text() { return Promise.resolve(this._s); }
    get size() { return this._s.length; }
  }
  class FilePF extends BlobPF {
    constructor(parts, name, opts) { super(parts, opts); this.name = name; }
  }
  class FormDataPF {
    constructor() { this._d = []; }
    append(k, v) { this._d.push([k, v]); }
    get(k) { return this._d.find(([key]) => key === k)?.[1] ?? null; }
    has(k) { return this._d.some(([key]) => key === k); }
    entries() { return this._d[Symbol.iterator](); }
  }
  class AbortControllerPF {
    constructor() {
      const listeners = [];
      this.signal = {
        aborted: false,
        reason: undefined,
        onabort: null,
        addEventListener(type, fn) {
          if (type === "abort" && typeof fn === "function") listeners.push(fn);
        },
        removeEventListener(type, fn) {
          if (type !== "abort") return;
          const i = listeners.indexOf(fn);
          if (i >= 0) listeners.splice(i, 1);
        },
        _listeners: listeners,
      };
    }
    abort(reason) {
      const s = this.signal;
      if (s.aborted) return;
      s.aborted = true;
      s.reason = reason;
      const ev = { type: "abort" };
      if (typeof s.onabort === "function") s.onabort(ev);
      for (const fn of s._listeners.slice()) fn(ev);
    }
  }
  class RequestPF {
    constructor(url, init) {
      this.url = String(url); this.method = init?.method ?? "GET";
      this.headers = new HeadersPF(init?.headers); this.body = init?.body ?? null;
    }
  }
  // A real Response (the per-request js-http-runner augments this with a streaming
  // body; the resident server reads .status/.headers/.body directly).
  class ResponsePF {
    constructor(body, init) {
      this.body = body ?? null;
      this.status = (init && init.status) || 200;
      this.statusText = (init && init.statusText) || "";
      this.headers =
        init && init.headers instanceof HeadersPF
          ? init.headers
          : new HeadersPF(init && init.headers);
      if (typeof this.body === "string" && !this.headers.has("content-type")) {
        this.headers.set("content-type", "text/plain;charset=UTF-8");
      }
    }
    get ok() {
      return this.status >= 200 && this.status < 300;
    }
    // Drain the body — a Uint8Array, a string, or a ReadableStream (a streamed fetch
    // response) — to bytes, so text()/json()/arrayBuffer() work either way.
    async _bytes() {
      const b = this.body;
      if (b == null) return new Uint8Array(0);
      if (b instanceof Uint8Array) return b;
      if (typeof b === "string") return new TextEncoderPF().encode(b);
      if (typeof b.getReader === "function") {
        const reader = b.getReader();
        const chunks = [];
        let total = 0;
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          chunks.push(value);
          total += value.length;
        }
        const out = new Uint8Array(total);
        let off = 0;
        for (const c of chunks) {
          out.set(c, off);
          off += c.length;
        }
        return out;
      }
      return new Uint8Array(0);
    }
    async arrayBuffer() {
      return (await this._bytes()).buffer;
    }
    async text() {
      return new TextDecoderPF().decode(await this._bytes());
    }
    async json() {
      return JSON.parse(await this.text());
    }
    // A shallow clone (shares the body) — enough for ai-sdk, which clones a response
    // to peek at it; matches the proven genius-wasmcloud bridge.
    clone() {
      return this;
    }
  }

  def("TextEncoder", TextEncoderPF);
  def("TextDecoder", TextDecoderPF);
  def("Headers", HeadersPF);
  def("URL", URLPF);
  def("URLSearchParams", URLSearchParamsPF);
  def("ReadableStream", ReadableStreamPF);
  def("Blob", BlobPF);
  def("File", FilePF);
  def("FormData", FormDataPF);
  def("AbortController", AbortControllerPF);
  def("Request", RequestPF);
  def("Response", ResponsePF);

  // setTimeout/clearTimeout via microtasks (QuickJS has no event-loop timers in this
  // single-threaded blocking model). The **two** microtask hops are deliberate, not a
  // delay: the Anthropic/ai-sdk pattern schedules a request-timeout abort *and* clears
  // it in a `.finally()` when the request settles — deferring two hops lets that
  // `clearTimeout` run first, so a completed request never spuriously aborts. (The
  // proven genius-wasmcloud bridge uses this exact shape; do not "upgrade" it to a real
  // timer without preserving that ordering.)
  if (!G.setTimeout) {
    let id = 0;
    const cancelled = new Set();
    G.setTimeout = (fn) => {
      const t = ++id;
      Promise.resolve().then(() => Promise.resolve().then(() => {
        if (!cancelled.has(t)) fn();
        cancelled.delete(t);
      }));
      return t;
    };
    G.clearTimeout = (t) => cancelled.add(t);
  }

  // crypto: secure randomness from the host (wasi:random) — getRandomValues + a v4
  // randomUUID — plus a `subtle` (SubtleCrypto) over native RustCrypto host calls
  // (digest / HMAC / AES-GCM). subtle methods are async (the Web Crypto contract);
  // the host calls underneath are synchronous + constant-time.
  if (!G.crypto) {
    // Normalise a BufferSource (ArrayBuffer | TypedArray) to a Uint8Array, and an
    // algorithm (string | { name, hash }) to its / its hash's name.
    const toBytes = (src) =>
      src instanceof Uint8Array
        ? src
        : src instanceof ArrayBuffer
          ? new Uint8Array(src)
          : new Uint8Array(src.buffer, src.byteOffset, src.byteLength);
    const algName = (a) => (typeof a === "string" ? a : a.name);
    const hashName = (a) => {
      const h = typeof a === "string" ? a : a && (a.hash || a.name);
      return typeof h === "string" ? h : h && h.name;
    };
    const aad = (algorithm) =>
      algorithm.additionalData ? toBytes(algorithm.additionalData) : new Uint8Array(0);

    G.crypto = {
      getRandomValues(view) {
        const bytes = __random_bytes(view.byteLength);
        new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(bytes);
        return view;
      },
      randomUUID() {
        const b = __random_bytes(16);
        b[6] = (b[6] & 0x0f) | 0x40; // version 4
        b[8] = (b[8] & 0x3f) | 0x80; // variant 10x
        let s = "";
        for (let i = 0; i < 16; i++) {
          if (i === 4 || i === 6 || i === 8 || i === 10) s += "-";
          s += b[i].toString(16).padStart(2, "0");
        }
        return s;
      },
      // SubtleCrypto: SHA digest, HMAC sign/verify, AES-GCM encrypt/decrypt. Keys are
      // modelled as plain objects holding the raw material (`__raw`) — the host
      // primitives are stateless over bytes. Unsupported algorithms/formats throw.
      subtle: {
        async digest(algorithm, data) {
          return __crypto_digest(algName(algorithm), toBytes(data)).buffer;
        },
        async importKey(format, keyData, algorithm, extractable, keyUsages) {
          if (format !== "raw") throw new Error("importKey: only 'raw' is supported");
          return {
            type: "secret",
            extractable: !!extractable,
            algorithm,
            usages: keyUsages || [],
            __raw: toBytes(keyData),
          };
        },
        async exportKey(format, key) {
          if (format !== "raw") throw new Error("exportKey: only 'raw' is supported");
          return key.__raw.slice().buffer;
        },
        async generateKey(algorithm, extractable, keyUsages) {
          const name = algName(algorithm);
          if (name === "AES-GCM") {
            const bits = algorithm.length || 256;
            const raw = G.crypto.getRandomValues(new Uint8Array(bits / 8));
            return { type: "secret", extractable: !!extractable, algorithm: { name, length: bits }, usages: keyUsages || [], __raw: raw };
          }
          if (name === "HMAC") {
            const hash = hashName(algorithm);
            const bytes = hash === "SHA-512" || hash === "SHA-384" ? 128 : 64; // hash block size
            const raw = G.crypto.getRandomValues(new Uint8Array(bytes));
            return { type: "secret", extractable: !!extractable, algorithm: { name, hash }, usages: keyUsages || [], __raw: raw };
          }
          throw new Error("generateKey: unsupported algorithm " + name);
        },
        async sign(algorithm, key, data) {
          if (algName(algorithm) !== "HMAC") throw new Error("sign: only HMAC is supported");
          return __crypto_hmac_sign(hashName(key.algorithm), key.__raw, toBytes(data)).buffer;
        },
        async verify(algorithm, key, signature, data) {
          if (algName(algorithm) !== "HMAC") throw new Error("verify: only HMAC is supported");
          return __crypto_hmac_verify(hashName(key.algorithm), key.__raw, toBytes(signature), toBytes(data));
        },
        async encrypt(algorithm, key, data) {
          if (algName(algorithm) !== "AES-GCM") throw new Error("encrypt: only AES-GCM is supported");
          return __crypto_aes_gcm_encrypt(key.__raw, toBytes(algorithm.iv), aad(algorithm), toBytes(data)).buffer;
        },
        async decrypt(algorithm, key, data) {
          if (algName(algorithm) !== "AES-GCM") throw new Error("decrypt: only AES-GCM is supported");
          return __crypto_aes_gcm_decrypt(key.__raw, toBytes(algorithm.iv), aad(algorithm), toBytes(data)).buffer;
        },
      },
    };
  }

  // fetch over the host's capability-gated wasi:http (via the `__fetch*` host calls).
  // The response body streams lazily — `__fetch_read` blocks the fiber until the next
  // chunk — so a token-by-token LLM response arrives incrementally; an AbortSignal
  // closes the stream early. A denied/failed request throws → the promise rejects.
  if (!G.fetch) {
    G.fetch = async (input, init) => {
      init = init || {};
      const isReq = input && typeof input === "object" && "url" in input;
      const url = isReq ? input.url : String(input);
      const method = String(init.method || (isReq && input.method) || "GET").toUpperCase();

      const hdr =
        init.headers instanceof HeadersPF
          ? init.headers
          : new HeadersPF(init.headers || (isReq ? input.headers : null));
      let headerStr = "";
      hdr.forEach((v, k) => {
        headerStr += k + ":" + v + "\n";
      });

      const raw = init.body != null ? init.body : isReq ? input.body : null;
      let bodyBytes;
      if (raw == null) bodyBytes = new Uint8Array(0);
      else if (raw instanceof Uint8Array) bodyBytes = raw;
      else if (raw instanceof ArrayBuffer) bodyBytes = new Uint8Array(raw);
      else bodyBytes = new TextEncoderPF().encode(typeof raw === "string" ? raw : String(raw));

      const head = __fetch(method, url, headerStr, bodyBytes);
      const status = parseInt(head[0], 10);
      const handle = parseInt(head[1], 10);
      const respHeaders = new HeadersPF();
      for (let i = 2; i + 1 < head.length; i += 2) respHeaders.set(head[i], head[i + 1]);

      let done = false;
      const close = () => {
        if (!done) {
          done = true;
          __fetch_close(handle);
        }
      };
      const body = new ReadableStreamPF({
        pull(controller) {
          if (done) return controller.close();
          const chunk = __fetch_read(handle);
          if (chunk == null) {
            done = true;
            controller.close();
          } else {
            controller.enqueue(chunk);
          }
        },
        cancel() {
          close();
        },
      });

      const signal = init.signal;
      if (signal) {
        if (signal.aborted) close();
        else if (signal.addEventListener) signal.addEventListener("abort", close);
      }
      return new ResponsePF(body, { status, headers: respHeaders });
    };
  }

  // process.env — capability-granted environment variables, read lazily from the host
  // (`__getenv` → wasi:cli/environment, scoped to this process's `env = [...]` grants).
  // An ungranted/absent key reads as `undefined`, matching Node's `process.env`. Enough
  // for the common config read (`process.env.ANTHROPIC_API_KEY`) the JS ecosystem expects.
  if (!G.process) G.process = {};
  if (!G.process.env) {
    G.process.env = new Proxy(
      {},
      {
        get(_t, key) {
          if (typeof key !== "string") return undefined;
          const value = __getenv(key);
          return value == null ? undefined : value;
        },
        has(_t, key) {
          return typeof key === "string" && __getenv(key) != null;
        },
      },
    );
  }
})();
