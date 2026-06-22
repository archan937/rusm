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

`rusm new --rust` (or `--lang go`) scaffolds a single Rust or Go component; `--protocol
ws|sse` a WebSocket or SSE handler. The scaffolded `rusm.toml` is the app manifest — see the
[configuration reference](/build-an-app/configuration) for every table and field
(`[[serve]]`, `[serve.routes]`, `[capabilities.<name>]`, `[components.<name>]`, env), and the
[`rusm` CLI reference](/build-an-app/cli) for the full command set.
