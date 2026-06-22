# Concept — the RUSM app model

A RUSM **app** is a project that declares some components and lets the runtime
build, load, supervise, and run them — so you write *source*, not glue.

## Layout

```
my-app/
├── rusm.toml          # node config + [components.<name>] (capability, resident)
│                       #   and/or [[serve]] listeners (protocol, listen; routes name handlers)
├── components/        # SOURCE — a cargo workspace, one crate per component
│   └── <name>/        # Rust (and TS via Bun + rquickjs, embedded — no jco)
└── wasm/              # BUILT, ready-to-run .wasm (also drop 3rd-party .wasm here)
```

The manifest refers to components **by name**; `./wasm/` is the enforced
load directory.

## Commands

- **`rusm new <name>`** — scaffolds a new app: a zero-dependency TypeScript HTTP
  component (`components/api/index.ts`, a default `Request`→`Response` handler), a
  `rusm.toml` with a `[[serve]]` entry, `.gitignore`, and a README. `rusm new hello
  && cd hello && rusm build && rusm serve` then `curl http://127.0.0.1:8080/` works
  end to end.
- **`rusm build`** — discovers `components/<name>/`, builds each with one
  toolchain (`cargo build --target wasm32-wasip2`, which componentizes — no jco,
  no cargo-component), and emits `./wasm/<name>.wasm`.
- **`rusm run`** — registers each manifest component under its capability profile so a
  route or a sibling can `spawn` it by name; the **`resident = true`** ones are also
  boot-spawned and supervised (auto-restarted on crash). Code can also spawn
  dynamically — both work together.
- **`rusm dev`** — build, then run, then watch `./components` and rebuild + reload
  on edit.
- **`rusm serve`** — hosts the `rusm.toml` **`[[serve]]`** listeners (`protocol` =
  `http` | `sse` | `ws`, `listen`, and — for HTTP/SSE — a `[serve.routes]` table) on
  real TCP ports. Each listener is pure: a routed HTTP/SSE listener names its handlers
  in `[serve.routes]` (each a `[components.<name>]` entry with its own capability); a WS or
  routes-less HTTP listener names its single handler with an optional `name`. Handlers
  load from `wasm/<name>.{wasm,js}` (HTTP and SSE via the `http_server` path, WS via
  `ws_server`). The node only serves; it never generates load.

## Environment — KISS, the Rust way

Env vars resolve **process environment first, then a `.env`** fallback (via
`dotenvy`; the real environment wins), exposed to guests through the standard
`wasi:cli/environment` — gated by the capability profile (default-deny: grant keys
or all). No wasmCloud-style `wasi:config/store`.

> Shipped in Phase 7 (`rusm-cli`).
