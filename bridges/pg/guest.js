// Canonical source: bridges/pg/guest.js — the pg bridge's JS guest binding (process-group
// tags). Synced into the js-runner (crates/rusm-wasm/js-runner/bridge/pg.js, which the
// js-http-runner also include_str!s) by `make sync-bridges`; edit this file, not the copy.
// `bridge_guest_in_sync` guards drift.
//
// pg ops surface as Process methods (joining/leaving a tag is a process operation), so this
// *augments* the existing `Process` global — it is eval'd AFTER process.js. Each op is wired
// only if the runner installed its host primitive (whereisTag-only on the http runner, for
// pub/sub publish; all four on the actor runner). killTag needs process-control at the host.

((P) => {
  const has = (fn) => typeof globalThis[fn] === "function";
  if (has("__register_tag")) P.registerTag = (tag) => __register_tag(tag);
  if (has("__unregister_tag")) P.unregisterTag = (tag) => __unregister_tag(tag);
  if (has("__whereis_tag")) P.whereisTag = (tag) => __whereis_tag(tag).map((p) => BigInt(p));
  if (has("__kill_tag")) P.killTag = (tag) => __kill_tag(tag);
})(globalThis.Process);
