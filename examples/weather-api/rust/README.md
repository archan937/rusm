# weather-api — Rust bridge host

A Rust bridge (`bridges/weather/host.rs`) called from a Rust HTTP handler. The host impl
compiles directly into the host binary — zero delegation, no actor, no JSON.

## Run

```sh
rusm build
rusm serve
curl http://127.0.0.1:8080/forecast/Amsterdam
curl http://127.0.0.1:8080/detailed/Amsterdam
```

## How it works

`rusm build` discovers `bridges/weather/`, regenerates `src/{bindings,bridges}.rs` and `wit/`,
vendors the contract into `components/api/`, and compiles the Rust guest. `rusm serve` runs
the host binary, which calls `rusm_cli::host::serve(.., bridges::extend)`.

See [`../../README.md`](../../README.md) for the full bridge model and all three flavours.
