# The rusm CLI

One binary, `rusm`, drives the whole lifecycle of a RUSM app. The arc:

```sh
rusm new myapp     # scaffold
cd myapp
rusm build         # components/* → ./wasm/*
rusm serve         # host them on real ports     (or)   rusm run   # run as processes
rusm dev           # build + run + watch & reload (iterate)
rusm attach        # live REPL into a running node
```

Config comes from `rusm.toml` (see **[configuration](/deep-dive/configuration)**);
the commands that start a node also accept the flags in the last section.

## `rusm new <name> [--rust|--lang ts|rust|go|generic] [--protocol http|sse|ws] [--template todo-board|weather] [--bridges]`

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
| `--template <name>` | _(none)_ | `todo-board`, `weather` — see below |
| `--bridges` | _(off)_ | scaffold a **custom-bridge** app — see below |

```sh
rusm new chat --protocol ws            # a TypeScript WebSocket echo
rusm new feed --protocol sse           # a TypeScript SSE stream
rusm new api  --rust                   # a Rust HTTP handler
rusm new api  --rust --protocol ws     # a Rust WebSocket handler
rusm new api  --lang go                # a Go HTTP handler
rusm new api  --lang go --protocol ws  # a Go WebSocket handler
```

What each cell scaffolds:

- **Rust HTTP** — a `#[rusm_rs::handlers] pub mod api { … }` component (each `pub fn` is a
  routable `fn(Request, Params) -> Response` action) **plus a `[serve.routes]` subtable** on
  that listener's `[[serve]]` entry in `rusm.toml`, mapping `"METHOD /path"` → `"api#action"`.
  No `main`, no router, no `wit/` dir — routing is declarative config.
- **Rust SSE** — a `sse::serve` handler (`open`/`message`/`close`, one sandboxed process per
  connection — the SSE twin of WS). Optionally path-routed via `[serve.routes]` (a bare
  handler-component value, no `#action`), reading path params from its connection context.
- **Rust WS** — a `ws::serve` handler (`open`/`message`/`close`, one sandboxed process
  per connection); same optional `[serve.routes]` as SSE.
- **TypeScript HTTP** — a zero-dependency web-standard
  `export default function handle(request): Response` (a `wasi:http` per-request
  component); it does its own dispatch, so no `[serve.routes]`.
- **TypeScript SSE** — the `rusm-ts` package's `export default sse({ open, message, close })`
  helper (one process per connection); optionally path-routed via `[serve.routes]`.
- **TypeScript WS** — the `rusm-ts` package's `export default websocket({ open, message, close })`
  helper; optionally path-routed via `[serve.routes]`.
- **Go HTTP** — a `web.NewHandlers()` component (each `h.Handle("name", …)` is a routable
  buffered action; normal Go, no bindings boilerplate) **plus a `[serve.routes]` subtable**,
  exactly like Rust — compiled via TinyGo.
- **Go SSE** — a `web.Sse{ Open, Message, Close }.Serve()` handler (one process per
  connection); optionally path-routed via `[serve.routes]`.
- **Go WS** — a `web.WebSocket{ Open, Message, Close }.Serve()` handler (one sandboxed
  process per connection); same optional `[serve.routes]` as SSE.
- **Generic (`--lang generic`)** — no source is generated; you drop a pre-built
  **wasip2** component into `components/api/` (a scaffolded `README.md` states the
  expected interface: `wasi:http/incoming-handler` for HTTP/SSE, a `rusm:runtime`
  actor for WS, or `wasi:cli/run` for a command). `rusm build` copies it into `wasm/`.

### `--template todo-board`

Instead of the minimal single-component starter, scaffold a **full example app** — the
collaborative todo board, the same one under [`examples/<lang>`](https://github.com/archan937/rusm/tree/main/examples):
five components (HTTP CRUD `api`, SSE `feed`, WebSocket `chat`, a `store` service, and a
`reporter` worker) wired together by process-group tags, with a web UI. `--protocol` does
not apply (the board brings its own listeners); choose the language with `--lang`.

```sh
rusm new board --template todo-board --lang go   # the whole app, in Go
cd board && rusm build && rusm serve              # open http://127.0.0.1:8080
```

### `--template weather`

Scaffold the **custom-bridge example** — a small host crate that registers the app's native
bridges (then serves), the example `weather` bridge (`bridges/weather/{bridge.wit,host.rs}`),
and a guest component that calls it as a typed import. It's the named, discoverable form of
the custom-bridge app, and how a guest reaches host code the platform doesn't provide — see
[Add your own functions](/build-an-app/add-your-own-functions). The host impl is always Rust
(the host *is* Rust); `--lang` chooses the **guest** language — **TypeScript, Rust, or Go**
(a TS guest calls the bridge too; the per-app js-runner is rebuilt with it compiled in).

```sh
rusm new forecast --template weather --lang ts    # a TS guest calling a native `weather` bridge
rusm new forecast --template weather --lang go    # …or Go, or Rust (the default)
cd forecast && rusm build && rusm serve
```

### `--bridges`

The same custom-bridge app as `--template weather`, as a flag — for *starting* a new app with
a bridge already wired in (defaults the guest to Rust). The two are interchangeable; pick
whichever reads better. `--bridges` can't be combined with `--template`.

```sh
rusm new weatherapp --bridges --lang go   # a Go guest calling a native `weather` bridge
```

## `rusm build`

Compile every `components/<name>/` into `./wasm/`, with **one toolchain each** — no
jco, no cargo-component:

- a **Rust** component (`Cargo.toml`) → `cargo build --target wasm32-wasip2` → `wasm/<name>.wasm`;
- a **TypeScript** component (`index.ts`) → `bun build --minify` → `wasm/<name>.js`,
  then **precompiled to QuickJS bytecode** → `wasm/<name>.qjsbc` (the runner skips parsing);
- a **Go** component (`go.mod`) → `tinygo build -target=wasip2 …` → `wasm/<name>.wasm`.
  See [guests: Rust, TypeScript & Go](/deep-dive/guests).
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
> repo-only tool — `rusm-bench start` (see [the dashboard](/about/benchmark-dashboard-and-observer)
> / `make dashboard`), not the installed `rusm`.

## `rusm kv`

`rusm kv <set|get|list|rm> …` — read and write the node's durable store (the `[node] store`
file) from the shell — chiefly
to **publish a dynamic bundle** that a `source = "kv:<bucket>/<key>"` (or a guest's
`spawn-from`) then loads: a compiled `.wasm` component or a JS bundle. The node must be
**stopped** (the store is single-writer, and a running node holds the lock).

```sh
rusm kv set plugins/greeter wasm/greeter.wasm   # publish a bundle (file → key)
rusm kv list plugins                            # greeter
rusm kv get plugins/greeter ./out.wasm          # read a key back to a file
rusm kv rm  plugins/greeter                      # delete a key
```

The `<bucket>/<key>` ref splits on the **first** `/`, so a key may contain slashes
(`plugins/v2/greeter` → bucket `plugins`, key `v2/greeter`). See
[dynamic WASM](/build-an-app/dynamic-wasm) for the publish → spawn flow.

## `rusm attach [target]`

Open a live REPL into a running node (defaults to `127.0.0.1:4000`; accepts
`host`, `host:port`, or a full `ws://` URL — local or remote). Watch the node's
live processes stream in (count + a per-process detail table), and toggle the
detail table. See [live attach](/deep-dive/live-attach).

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
| `--listen <addr>` | `node start`, `run`, `serve`, `dev` | Override the node's `[node] listen` attach (WebSocket) address — most useful with `node start`, which exposes it. |

> `rusm new` takes the app name; `rusm attach` takes the target as a positional
> argument; `rusm build` takes no flags.

Two flags are **global** — they work with any command, or none:

| Flag | Meaning |
| --- | --- |
| `-h`, `--help` | The top-level help, or a command's (`rusm <command> --help`). |
| `-V`, `--version` | Print the `rusm` version (e.g. `rusm 0.4.2`). |
