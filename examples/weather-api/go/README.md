# weather-api — Go bridge host

A Go bridge (`bridges/weather/host.go`) called from a Go HTTP handler. `rusm build` generates
`_runner.go`, compiles the whole bridge package with TinyGo to `wasm/bridge-weather.wasm`,
and generates the Rust delegation shim + host binary entry point.

## Run

```sh
rusm build
rusm serve
curl http://127.0.0.1:8080/forecast/Amsterdam
curl http://127.0.0.1:8080/detailed/Amsterdam
```

## How it works

`rusm build` discovers `bridges/weather/host.go`, generates `_runner.go` and `go.mod`,
then TinyGo compiles `bridges/weather/` → `wasm/bridge-weather.wasm`. The Go runner is
registered as a resident actor at startup; each call crosses the actor wire in ~1–10µs.
WIT record params arrive as `json.RawMessage` — your `host.go` function unmarshals into
its own struct.

See [`../../README.md`](../../README.md) for the full bridge model and all three flavours.
