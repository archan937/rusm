# URL shortener — TypeScript

A self-routing TypeScript HTTP handler over durable `kv`. One component, one file, the
minimal complete RUSM app. The runnable companion to the docs guide
[*A URL shortener*](https://archan937.github.io/rusm/build-an-app/url-shortener).

## Run it

```sh
rusm build         # bundles components/api/index.ts via Bun
rusm serve         # api → http://127.0.0.1:8080

curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'   # → /1
curl -i 127.0.0.1:8080/1   # → 302 location: https://rusm.dev/docs
```

## How it works

`components/api/index.ts` exports a `fetch` handler — it routes itself (`POST /shorten` /
`GET /:code`) so no `[serve.routes]` is needed in the manifest. Each request runs in its
own sandboxed Wasm instance; the `kv` bucket persists the `code → URL` map across restarts
(`links.redb`).

Uses **published** deps (`rusm-ts@^0.5.0`) — copy this directory out of the repo and it
builds on its own.

See [`../README.md`](../README.md) for the Rust and Go variants.
