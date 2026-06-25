# Quick start

From nothing to a live server in four commands:

```sh
rusm new hello && cd hello   # scaffold a TS HTTP component + rusm.toml
rusm build                   # components/ → wasm/
rusm serve                   # → http://127.0.0.1:8080
curl http://127.0.0.1:8080/  # "Hello from RUSM 👋"
```

## Scaffold a real app — the TODO board

Want a real app instead of hello world? Scaffold the full **TODO board** — HTTP CRUD, a
live SSE feed, WebSocket Chat, and a resident `store` service driven by a worker — in
TypeScript, Rust, or Go:

```sh
rusm new board --template todo-board   # add --lang rust or --lang go (default: ts)
cd board && rusm build && rusm serve   # → open http://127.0.0.1:8080
```

Or scaffold the **weather** template — a native host function (a "custom bridge") called from
your guest, in any language — `rusm new forecast --template weather --lang ts`; see
[Add your own functions](/build-an-app/add-your-own-functions).

`--lang rust` (or `--lang go`) scaffolds a single Rust or Go component instead of TypeScript;
`--protocol ws` (or `sse`) scaffolds a WebSocket or SSE handler instead of HTTP.

## Add components and bridges

Once you have a project, `rusm generate` adds to it without touching anything else:

```sh
rusm generate component payments --lang rust --protocol http  # add a Rust HTTP component
rusm generate component feed --protocol sse                   # add a TS SSE component
rusm generate bridge mailer --lang ts                         # add a TS host bridge
```

The scaffolded `rusm.toml` is your app manifest — see the
[configuration reference](/deep-dive/configuration) for every table and field
(`[[serve]]`, `[serve.routes]`, `[capabilities.<name>]`, `[components.<name>]`, env), and the
[`rusm` CLI reference](/build-an-app/the-rusm-cli) for the full command set.

Next: **[Build an app](/build-an-app/url-shortener)** walks the whole path — writing a component
in your language, serving it over HTTP/WS/SSE, and the common patterns (calling another
component, stateful services, broadcast, supervision).
