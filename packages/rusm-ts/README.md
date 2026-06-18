# rusm — the TypeScript guest API

The guest API for [RUSM](https://github.com/archan937/rusm) **TypeScript components**:
the `Process` actor API, the `spawn` typed-client, the `websocket()` serving helper, and
the types. The Rust twin is the [`rusm-rs`](https://crates.io/crates/rusm-rs) crate —
they share one JSON wire and interoperate.

```sh
bun add rusm-ts
```

```ts
import { Process, spawn } from "rusm-ts";
import type { Calc } from "../calc";       // type-only: the service's published contract

const calc = spawn<Calc>("calc");          // spawn a service by name, typed client
const sum = await calc.add(2, 3);          // a call
for await (const n of calc.countTo(3)) {}  // a stream
await calc.work((pct) => updateBar(pct));  // a callback (stays in the caller)
```

The js-runner injects `Process`/`spawn` as globals and polyfills the Web APIs (`URL`,
`TextEncoder`, `Headers`, `ReadableStream`, `AbortController`, `crypto`, `console`, and a
capability-gated streaming `fetch`); this package re-exports `Process`/`spawn`/`supervise`
as a normal module and ships the types. `receive`/`Stream.read` are async (`await`); pids
are `bigint`.

## Serving

A component is served by declaring it in `rusm.toml`; you just write the handler. HTTP/SSE
are zero-dependency web-standard handlers:

```ts
export default function handle(request: Request): Response {
  return new Response("hello\n", { headers: { "content-type": "text/plain" } });
}
```

WebSockets use the `websocket()` helper — one instance serves every connection, reply with
`socket.send(…)`:

```ts
import { websocket } from "rusm-ts";

export default websocket({
  open(socket) {
    socket.send("welcome\n");
  },
  message(socket, data) {
    socket.send(data); // echo this connection's frame
  },
  close(socket) {
    // disconnect — clean or dropped (optional)
  },
});
```

## Building

`rusm build` bundles each `components/<name>/index.ts` with Bun → `wasm/<name>.js`
(precompiled to QuickJS bytecode), run on the shared rquickjs runner. Add the standard
`DOM` lib to your `tsconfig.json` (`"lib": ["ES2022", "DOM"]`) so the polyfilled Web APIs
are typed. See the [RUSM docs](https://archan937.github.io/rusm/) and `rusm new` to
scaffold an app.
