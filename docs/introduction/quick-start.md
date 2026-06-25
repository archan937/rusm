# Quick start

From nothing to a live server in four commands:

```sh
rusm new hello               # new project with a TypeScript HTTP component
cd hello
rusm build                   # components/ → wasm/
rusm serve                   # → http://127.0.0.1:8080
curl http://127.0.0.1:8080/  # "Hello from RUSM 👋"
```

`--lang rust` or `--lang go` scaffolds a Rust or Go component instead of TypeScript;
`--protocol ws` or `--protocol sse` scaffolds a WebSocket or SSE handler instead of HTTP.
See the [`rusm` CLI reference](/build-an-app/the-rusm-cli) for the full command set.

## What just happened?

Four commands, and you're serving real HTTP from a sandboxed WebAssembly component. Here's
what each one did:

- **`rusm new hello`** scaffolded a project: a `rusm.toml` manifest with a `[[serve]]` entry,
  a TypeScript component at `components/api/index.ts` (a web-standard `fetch` handler), a
  `.gitignore`, and a README — a working app, not an empty skeleton.
- **`rusm build`** compiled that component to `./wasm/` — Bun bundles the TypeScript and it's
  precompiled to QuickJS bytecode (no jco, no per-component engine). A Rust or Go component
  would compile to `wasm32-wasip2` instead; same `wasm/` output either way.
- **`rusm serve`** bound the `[[serve]]` listener to a real TCP port. Each request runs in a
  **fresh sandboxed instance** — process-per-request, so one slow or crashing request can
  never block or take down another.
- **`curl`** hit that port and got the component's response back over a real socket.

There's no resident request handler, no thread pool to size, no framework to learn — you
wrote a `fetch` function and RUSM made it a server. From here, the same model scales to
WebSocket and SSE, multiple components, supervision, and a cluster.

## Scaffold a real app — the TODO board

Want a real app instead of hello world? Scaffold the full **TODO board** — HTTP CRUD, a
live SSE feed, WebSocket chat, and a resident `store` service driven by a worker — in
TypeScript, Rust, or Go:

```sh
rusm new board --template todo-board   # add --lang rust or --lang go (default: ts)
cd board && rusm build && rusm serve   # → open http://127.0.0.1:8080
```

Or scaffold the **weather** template — a native host function (a "custom bridge") called
from your guest, in any language — `rusm new forecast --template weather --lang ts`; see
[Add your own functions](/build-an-app/add-your-own-functions).

## Add components and bridges

Once you have a project, `rusm generate` adds to it without touching anything else:

```sh
rusm generate component payments --lang rust --protocol http  # add a Rust HTTP component
rusm generate component feed --protocol sse                   # add a TS SSE component
rusm generate bridge mailer --lang ts                         # add a TS host bridge
```

## Where to go next

- **[Build an app](/build-an-app/url-shortener)** — walks the full path: write a component,
  serve it over HTTP/WS/SSE, call other components, supervise, broadcast.
- **[Configuration reference](/deep-dive/configuration)** — every `rusm.toml` table and
  field: `[[serve]]`, `[serve.routes]`, `[capabilities.<name>]`, `[components.<name>]`, env.
- **[Write a component](/build-an-app/write-a-typescript-component)** — TypeScript, Rust,
  or Go, with the full Process API and serving patterns.
