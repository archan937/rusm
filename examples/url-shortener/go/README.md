# URL shortener — Go

A Go HTTP handler over durable `kv` (TinyGo → `wasm32-wasip2`). One component, declarative
routing in `rusm.toml`, the minimal complete RUSM Go app. The runnable companion to the docs
guide [*A URL shortener*](https://archan937.github.io/rusm/build-an-app/url-shortener).

## Run it

Requires **Go** and **TinyGo** installed:

```sh
rusm build         # TinyGo → wasm32-wasip2
rusm serve         # api → http://127.0.0.1:8080

curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'   # → /1
curl -i 127.0.0.1:8080/1   # → 302 location: https://rusm.dev/docs
```

## How it works

`components/api/main.go` registers two actions — `shorten` and `expand` — with
`web.NewHandlers()`, routed declaratively by `[serve.routes]` in `rusm.toml`. Each request
runs in its own sandboxed Wasm instance; the `kv` bucket persists the `code → URL` map across
restarts (`links.redb`).

Uses **published** deps (`rusm-go@v0.4.0`) — copy this directory out of the repo and it builds
on its own.

See [`../README.md`](../README.md) for the TypeScript and Rust variants.
