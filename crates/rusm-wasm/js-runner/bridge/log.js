// Canonical source: bridges/log/guest.js — the log bridge's JS guest binding (the
// `console.*` → platform-logger polyfill for QuickJS). Synced into the js-runner
// (crates/rusm-wasm/js-runner/bridge/log.js, which the js-http-runner also include_str!s)
// by `make sync-bridges`; edit this file, not the copy. `bridge_guest_in_sync` guards drift.
//
// A guest just calls console.* and gets the platform look: the host stamps the time, the
// process's component name + pid, and the severity colour, and gates by the node `[log]
// level` — so console maps each method to a level and forwards to the host `__log` op. A
// runner without the actor world has no `__log`; we fall back to raw stderr (`__print`)
// with a `[level]` prefix. Eval'd before the rest so guest + bridge code can log freely.

(() => {
  const G = globalThis;
  if (G.console) return;
  // bigint (pids!) and undefined have no JSON form — String() them; JSON the rest.
  const show = (x) =>
    typeof x === "string" ? x
    : typeof x === "bigint" || x === undefined ? String(x)
    : JSON.stringify(x);
  const fmt = (...a) => a.map(show).join(" ");
  const log = G.__log;
  const print = G.__print ?? (() => {});
  const at = log
    ? (level) => (...a) => log(level, fmt(...a))
    : (level) => (...a) => print(level === "info" ? fmt(...a) : `[${level}] ` + fmt(...a));
  G.console = {
    log: at("info"),
    info: at("info"),
    warn: at("warn"),
    error: at("error"),
    debug: at("debug"),
  };
})();
