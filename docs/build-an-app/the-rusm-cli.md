# The `rusm` CLI

Most runtimes hand you primitives and step away. RUSM gives you the full lifecycle:
scaffold a project, compile every component to Wasm, serve them on real ports, watch
and reload while you iterate, publish bundles to the durable store, and attach to any
live node — all through one binary.

```sh
rusm new myapp     # scaffold a project
cd myapp
rusm build         # components/* → ./wasm/*
rusm serve         # host them on real TCP ports
rusm dev           # build + run + watch & reload (the inner loop)
rusm attach        # live REPL into a running node
```

Config comes from `rusm.toml` (see **[configuration](/deep-dive/configuration)**);
commands that start a node also accept the flags in the last section.

## `rusm new <name> [--lang ts|rust|go|generic] [--protocol http|sse|ws] [--template …] [--bridges]`

Start from zero. `rusm new` scaffolds a complete project — component source, a
`rusm.toml` with a `[[serve]]` entry, `.gitignore`, and a README — so the first
thing you run actually works. No boilerplate to understand, no config to figure out.

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
| `--template <name>` | _(none)_ | `todo-board`, `weather`, `mailer` — see below |
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
collaborative todo board, the same one under [`examples/todo-board/<lang>`](https://github.com/archan937/rusm/tree/main/examples/todo-board):
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

### `--template mailer`

Scaffold a **mailer bridge** app — a TypeScript host bridge that sends transactional email
via [Resend](https://resend.com), registered as a resident actor. The same structure as
`--template weather` but using a TS host (`host.ts`) instead of Rust, demonstrating the
TS bridge path: `rusm build` generates the Rust delegation shim, the TS runner, and the
host binary; the guest calls `mailer.send()` as a plain typed import. Set `RESEND_API_KEY`
in `.env` before serving. `--lang` chooses the **guest** language — **TypeScript, Rust, or Go**.

```sh
rusm new notifier --template mailer --lang ts    # a TS guest calling the mailer bridge
rusm new notifier --template mailer --lang go    # …or Go, or Rust
cd notifier && rusm build && rusm serve
```

The live examples live at [`examples/mailer/`](https://github.com/archan937/rusm/tree/main/examples/mailer) — three flavours, one per bridge host language.

### `--bridges`

The same custom-bridge app as `--template weather`, as a flag — for *starting* a new app with
a bridge already wired in (defaults the guest to Rust). The two are interchangeable; pick
whichever reads better. `--bridges` can't be combined with `--template`.

```sh
rusm new weatherapp --bridges --lang go   # a Go guest calling a native `weather` bridge
```

## `rusm generate component|bridge|authentication <name> [options]`

`rusm new` starts from zero. `rusm generate` grows an existing project — it adds one
component, bridge, or auth hook without touching anything else already in the project.

```sh
rusm generate component payments --lang rust --protocol http  # add a Rust HTTP component
rusm generate component feed --protocol sse                   # add a TS SSE component
rusm generate bridge mailer --lang ts                         # add a TS host bridge
rusm generate authentication jwt --lang rust                  # add a serving auth hook
```

### `component <name> [--lang ts|rust|go] [--protocol http|sse|ws]`

Creates `components/<name>/` with the appropriate source files and appends the correct
`rusm.toml` entry — a `[[serve]]` block for TypeScript and for Rust/Go SSE/WS (standalone
listeners), or a `[components.<name>]` section for Rust/Go HTTP handlers that route via
`[serve.routes]` on an existing listener.

Errors if `components/<name>/` already exists or if `<name>` is already declared in
`rusm.toml`, so you can never silently clobber an existing component.

### `bridge <name> [--lang ts|rust|go]`

Creates `bridges/<name>/bridge.wit` (the WIT contract for the bridge) and
`bridges/<name>/host.<ext>` (the host implementation in the chosen language), then appends
an instructional comment to `rusm.toml` showing the exact `[capabilities.<name>]` snippet
to grant the bridge to a component:

```toml
# Bridge 'mailer' — grant it in a capability to call it from a guest:
#   [capabilities.my-cap]
#   inherits = "sandboxed"
#   bridges = ["mailer"]
# Then set capability = "my-cap" on the component(s) that import it.
```

See [Add your own functions](/build-an-app/add-your-own-functions) for the full bridge workflow.

### `authentication <name> [--lang ts|rust|go]`

Creates `auth/<name>/host.<ext>` (a starter `authenticate` that denies by default — fail-closed),
then appends a comment to `rusm.toml` showing how to apply it to a listener:

```toml
# Auth hook 'jwt' — apply it to a listener by adding to its [[serve]] entry:
#   authentication = "jwt"
```

The hook runs before each request: it validates the request host-side and seeds the request's
host-only claims context (read by a [multi-tenant bridge](/build-an-app/multi-tenant-bridges)),
or rejects it with `401`. A Rust hook compiles into the host binary; a TS/Go hook runs as a
supervised resident runner. See [Multi-tenant bridges](/build-an-app/multi-tenant-bridges) for
the full flow.

## `rusm build`

There is no build system config to write. RUSM inspects each `components/<name>/` directory,
detects the toolchain from the layout, and compiles it with the right tool:

- a **Rust** component (`Cargo.toml`) → `cargo build --target wasm32-wasip2` → `wasm/<name>.wasm`
- a **TypeScript** component (`index.ts`) → `bun build --minify` → `wasm/<name>.js`, then
  **precompiled to QuickJS bytecode** → `wasm/<name>.qjsbc` (the runner skips JS parsing at spawn time)
- a **Go** component (`go.mod`) → `tinygo build -target=wasip2 …` → `wasm/<name>.wasm`
  (see [guests: Rust, TypeScript & Go](/deep-dive/guests))
- a **generic** component (a pre-built `.wasm`, no `Cargo.toml`/`index.ts`) → copied into
  `wasm/<name>.wasm` as-is; prefers `<name>.wasm`, a lone `.wasm` also works, multiple
  `.wasm` files are an error (name the one to ship `<name>.wasm`)

Emits a clear error if Bun or the `wasm32-wasip2` target is missing.

## `rusm run`

Host named components so other parts of your app — or other nodes — can `spawn` them.
`rusm run` loads every `[components.<name>]` entry from `./wasm/`, registers each under its
declared capability profile, and boot-spawns any `resident = true` entries under supervision.
Use it when the app's value is in the services it provides, not in serving HTTP directly.

```sh
rusm run
# running 2 component(s): calc, commander
```

## `rusm serve`

Host HTTP/WS/SSE listeners on real TCP ports. Every `[[serve]]` entry in `rusm.toml` becomes
a bound port; RUSM dispatches each incoming connection using the ephemeral model it was built
around — a **fresh sandboxed instance per request** for HTTP/SSE (routed through
`[serve.routes]` if declared), a **fresh sandboxed process per connection** for WS. A crash
drops exactly one unit; head-of-line blocking is impossible by construction.

Waits for Ctrl-C. The node only serves; drive load out-of-process with `rusm-loadtest`.

```sh
rusm serve
# serving 1 endpoint(s):
#   api              http://127.0.0.1:8080
```

## `rusm dev`

The inner loop: `build` → `run` → **watch `./components` and rebuild + hot-reload on any
edit**. Edit a file; RUSM detects the change (a dependency-free mtime scan), recompiles
that component, and reloads it — without restarting the node or other components.

```sh
rusm dev
# running 2 component(s); watching ./components — edit to reload, Ctrl-C to stop
# change detected — rebuilding…
```

## `rusm node start`

The production-ready variant of `rusm run`: hosts all `[components.<name>]` entries **and**
exposes the observe/attach WebSocket endpoint so `rusm attach` (or the dashboard) can
connect to the live node. Use `rusm node start` whenever you want visibility into a running
system; use `rusm run` for fully headless deployments.

```sh
rusm node start
# rusm node listening on ws://127.0.0.1:4000 (2 component(s), 20 Hz)
# attach with:  rusm attach 127.0.0.1:4000
```

> The **benchmark/observer node** behind the live dashboard is a separate,
> repo-only tool — `rusm-bench start` (see [the dashboard](/about/benchmark-dashboard-and-observer)
> / `make dashboard`), not the installed `rusm`.

## `rusm kv`

Publish dynamic bundles to the node's durable store so a `source = "kv:<bucket>/<key>"`
component or a guest's `spawn-from` can load them at runtime. The node must be **stopped**
(the store is single-writer; a running node holds the lock).

```sh
rusm kv set plugins/greeter wasm/greeter.wasm   # publish a bundle (file → key)
rusm kv list plugins                            # greeter
rusm kv get plugins/greeter ./out.wasm          # read a key back to a file
rusm kv rm  plugins/greeter                     # delete a key
```

The `<bucket>/<key>` ref splits on the **first** `/`, so a key may contain slashes
(`plugins/v2/greeter` → bucket `plugins`, key `v2/greeter`). See
[dynamic WASM](/build-an-app/dynamic-wasm) for the publish → spawn flow.

## `rusm attach [target]`

Observe — and script — a live node without stopping it. `rusm attach` opens a REPL into a
running node's process stream — defaults to `127.0.0.1:4000`; accepts `host`, `host:port`,
or a full `ws://` URL — local or remote. See [live attach](/deep-dive/live-attach).

```sh
rusm attach                 # local node
rusm attach 10.0.0.7:4000   # a remote node
# attached — type `help` for commands
> detail off                # just the live count, no per-process table
```

Any line that isn't a built-in command is **evaluated as JavaScript** against the live
node — a stateful shell (bindings persist across lines) with the full `Process` API, so
you can inspect, message, kill, and `connect()` to processes from the prompt:

```sh
> p = Process.whereis("store")
43
> await connect("store").list()
[{"id":1,"text":"ship the docs"}]
```

JS eval is **local-only** (loopback clients only) until the attach channel is
authenticated; a remote attach can still observe. See **[the live REPL](/deep-dive/the-live-repl)**
for the full tour.

## Flags

The **node-starting commands** (`node start`, `run`, `serve`, `dev`) accept these flags,
layered over `rusm.toml`:

| Flag | Commands | Meaning |
| --- | --- | --- |
| `--config <file>` | `node start`, `run`, `serve`, `dev` | Use a specific manifest instead of `./rusm.toml`. |
| `--listen <addr>` | `node start`, `run`, `serve`, `dev` | Override the node's `[node] listen` attach (WebSocket) address — most useful with `node start`, which exposes it. |

> `rusm new` and `rusm generate` take the app/component name as a positional argument;
> `rusm attach` takes the target as a positional argument; `rusm build` takes no flags.

Two flags are **global** — they work with any command, or none:

| Flag | Meaning |
| --- | --- |
| `-h`, `--help` | The top-level help, or a command's (`rusm <command> --help`). |
| `-V`, `--version` | Print the `rusm` version (e.g. `rusm 0.6.0`). |
