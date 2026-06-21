// Canonical source: bridges/streams/guest.js — the stream bridge's JS guest binding (the
// cross-process byte Stream for QuickJS, over the host `__stream_*` ops). Synced into the
// js-runner (crates/rusm-wasm/js-runner/bridge/streams.js) by `make sync-bridges`; edit this
// file, not the copy. `bridge_guest_in_sync` guards drift.
//
// `write` accepts a string (UTF-8) or a Uint8Array; `read` resolves to a Uint8Array or null
// at end-of-stream (host None → undefined → null). The host call still suspends the whole
// instance's fiber (freeing the Tokio worker); the Promise is driven by the QuickJS job
// queue. `Process.openStream`/`acceptStream` (process.js) construct these — so this is
// eval'd before process.js, exposing `Stream` as a global it can reference.

globalThis.Stream = class Stream {
  constructor(handle) {
    this.handle = handle;
  }
  // write accepts a string (UTF-8) or a Uint8Array.
  write(chunk) {
    return typeof chunk === "string"
      ? __stream_write_text(this.handle, chunk)
      : __stream_write(this.handle, chunk);
  }
  close() {
    __stream_close(this.handle);
  }
  // Resolves to a Uint8Array, or null at end-of-stream (host None → undefined → null).
  read() {
    const c = __stream_read(this.handle);
    return Promise.resolve(c === undefined ? null : c);
  }
};

// The RPC layer (rpc.js) tags Stream instances by this reference.
globalThis.__rusm_Stream = globalThis.Stream;
