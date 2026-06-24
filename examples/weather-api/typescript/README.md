# weather-api — TypeScript bridge host

A TypeScript bridge (`bridges/weather/host.ts`) called from a TypeScript WebSocket handler.
`rusm build` generates the Rust delegation shim, the TS runner, and the host binary.

## Run

```sh
bun install
rusm build
rusm serve
# send a city over WebSocket, get the forecast back
bun -e 'const w=new WebSocket("ws://127.0.0.1:8080");w.onopen=()=>w.send("Amsterdam");w.onmessage=e=>{console.log(e.data);process.exit(0)}'
```

## How it works

`rusm build` discovers `bridges/weather/host.ts`, generates the Rust delegation shim
(`src/bridge_weather_delegate.rs`), the TS runner (`bridges/weather/_runner.ts`), and the
host binary entry point (`src/main.rs`). Bun bundles the runner to `wasm/bridge-weather.js`.
At runtime, the runner is a resident actor; each bridge call crosses the actor wire in ~1–10µs.

See [`../../README.md`](../../README.md) for the full bridge model and all three flavours.
