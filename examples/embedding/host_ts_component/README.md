# host_ts_component — a TypeScript guest as a RUSM process

Shows **rusm-ts**: a TypeScript guest runs as a first-class, sandboxed RUSM
process, message-passing with the rest of the node — no per-component Wasm build.

```sh
cargo run -p rusm-bench --example host_ts_component
```

## What it shows

A TS component is **plain TypeScript** bundled by Bun and run on the shared
rquickjs **js-runner**. `WasmRuntime::spawn_js(bundle)` hands the bundle to a
fresh, isolated, default-deny process that gets the `Process` actor API (and the
Web API polyfills the runner installs). The worker here receives a reply-to pid,
labels itself `ts-worker`, and answers — exactly like an Erlang process.

## How it maps to a real app

In an app you don't embed the bundle; you write source and let RUSM build it:

```
my-app/
├── rusm.toml                 # [components.worker] capability = "sandboxed"
└── components/worker/index.ts
```

```ts
import { Process } from "rusm-ts";
const replyTo = await Process.receiveText();
Process.setLabel("ts-worker");
Process.send(replyTo, `pong from ${Process.self()}`);
```

`rusm build` runs `bun install` then bundles the `index.ts` with `bun build
--format=cjs` → `wasm/worker.js`, and `rusm run` loads `.js` artifacts on the
js-runner under the declared capability profile. (A Rust component builds to
`wasm/<name>.wasm` instead — same manifest, same loader.) `Process`/`spawn` and the
types come from the [`rusm-ts`](../../../packages/rusm-ts) package. See the full [`typescript`](../../typescript) todo-board example.
