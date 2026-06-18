# Reference — the `rusm` CLI

One binary, `rusm`, drives the whole lifecycle of a RUSM app. The arc:

```sh
rusm new myapp     # scaffold
cd myapp
rusm build         # components/* → ./wasm/*
rusm serve         # host them on real ports     (or)   rusm run   # run as processes
rusm dev           # build + run + watch & reload (iterate)
rusm attach        # live REPL into a running node
```

Config comes from `rusm.toml` (see **[configuration](./reference-configuration)**);
the commands that start a node also accept the flags in the last section.

## `rusm new <name> [--rust] [--protocol http|sse|ws]`

Scaffold a new app in `./<name>` — a component, a `rusm.toml` with a `[[serve]]`
entry, `.gitignore`, and a README. From nothing to a live server in three commands.

```sh
rusm new hello && cd hello
rusm build
rusm serve              # → http://127.0.0.1:8080
```

Pick the **language** and **protocol** — a 3×3 matrix, all generating *pure handler
code* (no `wit-bindgen`/`export!`, no `Process` frame plumbing):

| Flag | Default | Choices |
| --- | --- | --- |
| `--rust` / `--lang <ts\|rust\|go\|generic>` | TypeScript | `ts`, `rust`, `go`, `generic` |
| `--protocol <p>` / `-p <p>` | `http` | `http`, `sse`, `ws` |

```sh
rusm new chat --protocol ws            # a TypeScript WebSocket echo
rusm new feed --protocol sse           # a TypeScript SSE stream
rusm new api  --rust                   # a Rust HTTP handler
rusm new api  --rust --protocol ws     # a Rust WebSocket handler
rusm new api  --lang go                # a Go HTTP handler
rusm new api  --lang go --protocol ws  # a Go WebSocket handler
```

What each cell scaffolds:

- **Rust HTTP/SSE** — a `#[rusm_rs::handlers] pub mod api { … }` component (each
  `pub fn` is a routable action; `fn(Request, Params) -> Response` for HTTP,
  `fn(Request, Params, Sse)` to stream SSE) **plus a `[serve.routes]` subtable** on
  that listener's `[[serve]]` entry in `rusm.toml`, mapping `"METHOD /path"` →
  `"api#action"`. No `main`, no router, no `wit/` dir — routing is declarative config.
- **Rust WS** — a `ws::serve` handler (`open`/`message`/`close`, one sandboxed process
  per connection); no `[serve.routes]`.
- **TypeScript HTTP/SSE** — a zero-dependency web-standard
  `export default function handle(request): Response` (a `wasi:http` per-request
  component); it does its own dispatch, so no `[serve.routes]`.
- **TypeScript WS** — the `rusm-ts` package's `export default websocket({ open, message, close })` helper.
- **Go HTTP/SSE** — a `web.NewHandlers()` component (each `h.Handle("name", …)` /
  `h.HandleSSE("name", …)` is a routable action; normal Go, no bindings boilerplate)
  **plus a `[serve.routes]` subtable**, exactly like Rust — compiled via TinyGo.
- **Go WS** — a `web.WebSocket{ Open, Message, Close }.Serve()` handler (one sandboxed
  process per connection); no `[serve.routes]`.
- **Generic (`--lang generic`)** — no source is generated; you drop a pre-built
  **wasip2** component into `components/api/` (a scaffolded `README.md` states the
  expected interface: `wasi:http/incoming-handler` for HTTP/SSE, a `rusm:runtime`
  actor for WS, or `wasi:cli/run` for a command). `rusm build` copies it into `wasm/`.

## `rusm build`

Compile every `components/<name>/` into `./wasm/`, with **one toolchain each** — no
jco, no cargo-component:

- a **Rust** component (`Cargo.toml`) → `cargo build --target wasm32-wasip2` → `wasm/<name>.wasm`;
- a **TypeScript** component (`index.ts`) → `bun build --minify` → `wasm/<name>.js`,
  then **precompiled to QuickJS bytecode** → `wasm/<name>.qjsbc` (the runner skips parsing);
- a **Go** component (`go.mod`) → `tinygo build -target=wasip2 …` → `wasm/<name>.wasm`.
  See [guests: Rust, TypeScript & Go](./concepts/guests).
- a **generic** component (a pre-built `.wasm`, no `Cargo.toml`/`index.ts`) → copied
  into `wasm/<name>.wasm` as-is. Prefers `<name>.wasm`; a lone `.wasm` also works, and
  several `.wasm` files are an error (name the one to ship `<name>.wasm`).

Emits a clear error if Bun / the `wasm32-wasip2` target is missing.

## `rusm run`

Load every `[components.<name>]` entry from `./wasm/` and register it under its
capability profile so a route or sibling can `spawn` it by name; the `resident = true`
entries are also boot-spawned and supervised. Waits for Ctrl-C. Loads `./.env` (process
env wins).

```sh
rusm run
# running 2 component(s): calc, commander
```

## `rusm serve`

Host every `[[serve]]` entry on its TCP `listen` address. Serving is always
ephemeral: **HTTP/SSE** run a fresh sandboxed instance per request (`http_server`,
dispatched through that listener's `[serve.routes]` table), **WS** runs one sandboxed process per
connection (`ws_server`). Prints each bound endpoint; waits for Ctrl-C. This is the
**server** side of a fair benchmark — the node only serves; drive load
out-of-process with `rusm-loadtest`.

```sh
rusm serve
# serving 1 endpoint(s):
#   api              http://127.0.0.1:8080
```

## `rusm dev`

`build` → `run` → **watch `./components`** and rebuild + hot-reload on any edit (a
dependency-free mtime scan). The fast inner loop.

```sh
rusm dev
# running 2 component(s); watching ./components — edit to reload, Ctrl-C to stop
# change detected — rebuilding…
```

## `rusm node start`

Start an **attachable node**: host the app's `[components.<name>]` (like `rusm run`)
**and** expose a live observe/attach endpoint on `listen`, so `rusm attach` can
watch the node's processes. The hosted components keep running until Ctrl-C.

```sh
rusm node start
# rusm node listening on ws://127.0.0.1:4000 (2 component(s), 20 Hz)
# attach with:  rusm attach 127.0.0.1:4000
```

> The **benchmark/observer node** behind the live dashboard is a separate,
> repo-only tool — `rusm-bench start` (see [the dashboard](./03-benchmark-dashboard)
> / `make dashboard`), not the installed `rusm`.

## `rusm attach [target]`

Open a live REPL into a running node (defaults to `127.0.0.1:4000`; accepts
`host`, `host:port`, or a full `ws://` URL — local or remote). Watch the node's
live processes stream in (count + a per-process detail table), and toggle the
detail table. See [live attach](./concepts/live-attach).

```sh
rusm attach                 # local node
rusm attach 10.0.0.7:4000   # a remote node
# attached — type `help` for commands
> detail off                # just the live count, no per-process table
```

## Flags

Applied by the node-starting commands (layered over `rusm.toml`):

| Flag | Commands | Meaning |
| --- | --- | --- |
| `--config <file>` | `node start`, `run`, `serve`, `dev` | Use a specific manifest instead of `./rusm.toml`. |
| `--listen <addr>` | `node start` | Override the node's attach (WebSocket) address. |

> `rusm new` takes the app name; `rusm attach` takes the target as a positional
> argument; `rusm build` takes no flags.
