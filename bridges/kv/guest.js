// Canonical source: bridges/kv/guest.js — the kv bridge's JS guest binding (the
// QuickJS runtime bridge over the host `__kv_*` primitives). Synced into the js-runner
// (crates/rusm-wasm/js-runner/bridge/kv.js, which the js-http-runner also include_str!s) by
// `make sync-bridges`; edit this file, not the copy. `bridge_guest_in_sync` guards drift.
//
// kv.bucket(name) returns a handle: get/set/delete/exists/list. Values are Uint8Array;
// set also accepts a string (UTF-8 encoded). get returns null when the key is absent. A
// denied or failed op throws (the host's message) — the same shape as fetch. The published
// TypeScript types for this surface live in the rusm-ts package (the Kv interface).

globalThis.kv = {
  bucket(name) {
    return {
      // Uint8Array, or null when absent (host None → undefined → null).
      get(key) {
        const v = __kv_get(name, key);
        return v === undefined ? null : v;
      },
      set(key, value) {
        __kv_set(name, key, typeof value === "string" ? new TextEncoder().encode(value) : value);
      },
      delete(key) {
        return __kv_delete(name, key);
      },
      exists(key) {
        return __kv_exists(name, key);
      },
      list() {
        return __kv_list(name);
      },
    };
  },
};
