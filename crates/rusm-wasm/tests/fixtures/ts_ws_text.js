var __defProp = Object.defineProperty;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __hasOwnProp = Object.prototype.hasOwnProperty;
function __accessProp(key) {
  return this[key];
}
var __toCommonJS = (from) => {
  var entry = (__moduleCache ??= new WeakMap).get(from), desc;
  if (entry)
    return entry;
  entry = __defProp({}, "__esModule", { value: true });
  if (from && typeof from === "object" || typeof from === "function") {
    for (var key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(entry, key))
        __defProp(entry, key, {
          get: __accessProp.bind(from, key),
          enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable
        });
  }
  __moduleCache.set(from, entry);
  return entry;
};
var __moduleCache;
var __returnValue = (v) => v;
function __exportSetter(name, newValue) {
  this[name] = __returnValue.bind(null, newValue);
}
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, {
      get: all[name],
      enumerable: true,
      configurable: true,
      set: __exportSetter.bind(all, name)
    });
};

// index.ts
var exports_ts_ws_text = {};
__export(exports_ts_ws_text, {
  default: () => ts_ws_text_default
});
module.exports = __toCommonJS(exports_ts_ws_text);

// ../../../../../packages/rusm-ts/index.ts
var g = globalThis;
var Process = g.Process;
var kv = g.kv;
var connectionInfo = () => {
  const raw = Process.connection?.() ?? null;
  const params = raw?.params ?? [];
  const headers = raw?.headers ?? [];
  return {
    method: raw?.method ?? "",
    path: raw?.path ?? "",
    query: raw?.query ?? "",
    params,
    headers,
    remoteAddr: raw?.remoteAddr ?? "",
    subprotocol: raw?.subprotocol ?? null,
    param: (name) => params.find(([k]) => k === name)?.[1],
    header: (name) => headers.find(([k]) => k.toLowerCase() === name.toLowerCase())?.[1]
  };
};
var websocket = (handlers) => {
  let info;
  const socket = (id) => ({
    id,
    info: info ??= connectionInfo(),
    send: (data) => Process.send(id, data),
    sendText: (text) => Process.sendText(text)
  });
  return {
    websocket: {
      open: (conn) => handlers.open?.(socket(conn)),
      message: (conn, data) => handlers.message(socket(conn), data),
      close: (conn) => handlers.close?.(socket(conn))
    }
  };
};

// index.ts
var ts_ws_text_default = websocket({
  message: (s, data) => {
    s.sendText(new TextDecoder().decode(data));
  }
});
