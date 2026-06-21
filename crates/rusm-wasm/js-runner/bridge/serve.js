// Canonical source: bridges/serve/guest.js — the serve bridge's JS guest binding (the
// per-connection WS/SSE handler controls). Synced into the js-runner
// (crates/rusm-wasm/js-runner/bridge/serve.js) by `make sync-bridges`; edit this file, not
// the copy. `bridge_guest_in_sync` guards drift.
//
// These surface as Process methods on a connection handler, so this *augments* the existing
// `Process` global — eval'd AFTER process.js. Wired only if the runner installed the host
// primitive (the actor js-runner does; the http runner has no per-connection handlers, so it
// neither installs them nor evals this file).

((P) => {
  const has = (fn) => typeof globalThis[fn] === "function";
  if (has("__connection")) P.connection = () => JSON.parse(__connection() ?? "null");
  if (has("__ws_send_text")) P.sendText = (text) => __ws_send_text(text);
  if (has("__ws_close")) P.wsClose = (code, reason) => __ws_close(code, reason ?? "");
  if (has("__sse_send"))
    P.sseSend = (data, event, id, retry) => __sse_send(data, event ?? "", id ?? "", retry ?? 0);
})(globalThis.Process);
