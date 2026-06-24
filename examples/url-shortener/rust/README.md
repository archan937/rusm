# URL shortener — Rust

A `#[rusm_rs::handlers]` HTTP component over durable `kv`. One component, declarative
routing in `rusm.toml`, the minimal complete RUSM Rust app. The runnable companion to the
docs guide [*A URL shortener*](https://archan937.github.io/rusm/build-an-app/url-shortener).

## Run it

Requires the `wasm32-wasip2` target (`rustup target add wasm32-wasip2` once):

```sh
rusm build         # cargo → wasm32-wasip2
rusm serve         # api → http://127.0.0.1:8080

curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'   # → /1
curl -i 127.0.0.1:8080/1   # → 302 location: https://rusm.dev/docs
```

## How it works

`components/api/src/lib.rs` is a `#[rusm_rs::handlers]` module with two actions — `shorten`
and `expand` — routed declaratively by `[serve.routes]` in `rusm.toml`. Each request runs in
its own sandboxed Wasm instance; the `kv` bucket persists the `code → URL` map across restarts
(`links.redb`).

Uses **published** deps (`rusm-rs = "0.4.2"`) — copy this directory out of the repo and it
builds on its own.

See [`../README.md`](../README.md) for the TypeScript and Go variants.
