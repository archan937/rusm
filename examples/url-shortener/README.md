# URL shortener

A tiny but complete RUSM app — `POST` a long URL, get a short code; visit the code, get
redirected — in **TypeScript, Rust, and Go**. It's the runnable companion to the docs guide
[*A URL shortener*](https://archan937.github.io/rusm/build-an-app/url-shortener).

One handler component (`api`) keeps the `code → URL` map in durable **`kv`**, so a shortened
link survives a restart. Each request runs in its own sandboxed WASM instance; there's no
shared state to corrupt.

Every variant uses **published** dependency specs (`rusm-ts@^0.7.0`, `rusm-rs = "0.7.0"`,
`rusm-go@v0.7.0`) — copy any one directory out of the repo and it builds on its own.

## Run it

```sh
cd typescript          # or: rust | go
rusm build
rusm serve             # api → http://127.0.0.1:8080

# shorten a URL (the body is the long URL; the reply is the short path):
curl -X POST 127.0.0.1:8080/shorten -d 'https://rusm.dev/docs'
# → /1

# follow the code (-i shows the redirect a browser would take):
curl -i 127.0.0.1:8080/1
# → HTTP/1.1 302 Found
# → location: https://rusm.dev/docs
```

| Variant | Toolchain |
| --- | --- |
| [`typescript`](./typescript/) | Bun bundles `components/api/index.ts` |
| [`rust`](./rust/) | `cargo` → `wasm32-wasip2` (`#[rusm_rs::handlers]`) |
| [`go`](./go/) | TinyGo → `wasm32-wasip2` |

## How it maps to the manifest

- `[[serve]]` + (Rust/Go) `[serve.routes]` — the listener and how each request finds a handler
  action. TypeScript self-routes, so its listener just names the `component`.
- `[components.api]` with the `stateful` capability — `kv` is default-deny, so the handler is
  granted `allow-storage`.
- `[node] store` — the on-disk file the durable `kv` lives in (created on first write).
