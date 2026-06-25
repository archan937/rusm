# Write a Go component

You write Go. RUSM compiles it with **TinyGo** to `wasm32-wasip2` and runs it as a
**sandboxed, supervised process** — isolated memory, capability-gated I/O, crash-recovered
by the supervisor. No hand-rolled bindings, no toolchain wrangling. Write idiomatic Go;
RUSM handles the build.

## Scaffold & run in 30 seconds

```sh
rusm new myapp --lang go   # scaffold a Go HTTP component
cd myapp
rusm build                 # TinyGo → wasm/api.wasm
rusm serve                 # live on http://127.0.0.1:8080
```

Want WebSocket or SSE instead?

```sh
rusm new myapp --lang go --protocol ws    # WebSocket component
rusm new myapp --lang go --protocol sse   # Server-Sent Events component
```

A component is a folder under `components/` with its own `go.mod` and `main.go`:

```
my-app/
├── rusm.toml
├── components/
│   └── api/
│       ├── go.mod          # module + rusm-go dep
│       └── main.go
└── wasm/                   # rusm build writes api.wasm here
```

## Two shapes

### Service — register typed handlers

Register handlers with `rusm.NewService()`; call `svc.Serve()` to start the dispatch loop.
A caller reaches it with the generic `Call[R]` function — a real cross-process message,
hidden behind a function call:

```go
// components/calc/main.go
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
    svc := rusm.NewService()
    svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
    svc.HandleStream("countTo", func(req rusm.Request, out rusm.Sink) error {
        n, _ := rusm.Arg[int](req, 0)
        for i := 1; i <= n; i++ { out.Send(i) }
        return nil
    })
    svc.Serve()
}
```

### One-shot — `rusm.Run`

Register an entry with `rusm.Run`; `main` stays empty (the runtime drives it). Runs once,
does the job, exits. Use `rusm.Spawn` + `rusm.Call[R]` to reach a service:

```go
// components/commander/main.go
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
    calc, _ := rusm.Spawn("calc")

    sum, _ := rusm.Call[int](calc, "add", 2, 3)
    fmt.Println("2 + 3 =", sum)   // → 5
}
```

## Declare in `rusm.toml`

```toml
[components.calc]
capability = "sandboxed"

[components.commander]
capability = "trusted"   # inherits allow-spawn
```

## Build & run

```sh
rusm build   # TinyGo: components/*/main.go → wasm32-wasip2 → wasm/*.wasm
rusm run     # spawn them per rusm.toml
rusm dev     # build + run, then watch ./components and hot-reload on every save
```

`rusm build` generates the WIT bindings TinyGo needs and drives the full compile. You
write plain Go; no manual `wit-bindgen` invocation.

## What `rusm-go` gives you

The full actor toolkit, idiomatic Go:

| | |
|---|---|
| `rusm.Self()` | this process's `Pid` |
| `rusm.Send(pid, msg)` / `SendBytes(pid, b)` | send a message |
| `rusm.Receive()` / `ReceiveBytes()` / `ReceiveString()` | wait for a message (parks the goroutine) |
| `rusm.Spawn("name")` | spawn a component by `rusm.toml` name |
| `rusm.Call[R](pid, op, args...)` | typed cross-process call |
| `rusm.Register("name")` / `Whereis("name")` | named registry |
| `rusm.RegisterTag("tag")` / `WhereisTag("tag")` | process-group tags |
| `rusm.SendAfter(pid, ms, msg)` / `CancelTimer(h)` | timers |
| `rusm.Monitor(pid)` / `Link(pid)` | lifecycle tracking |

Logging is the standard `log` / `log/slog` packages — routed to the node's unified log
stream by the SDK automatically. The host stamps the time, `component#pid`, and severity.
No setup, no `allow-stdio`.

::: tip Same wire as TypeScript and Rust
A Go service and a TypeScript or Rust caller interoperate out of the box — same JSON wire.
Mix languages freely; each component stays isolated behind its own capability profile.
:::

## Go deeper

- [Call another component](/build-an-app/call-another-component) — `Call[R]`, `connect` to a resident, `CallTimeout` for deadlines
- [Serve HTTP / WS / SSE](/build-an-app/serve-http) — `web.Handlers` for routed HTTP, `web.WebSocket`, `web.Sse`
- [Coordinate & supervise](/build-an-app/coordinate-and-supervise) — links, monitors, in-guest supervisor
- [Runnable todo-board](https://github.com/archan937/rusm/tree/main/examples/todo-board/go) — service + one-shot + streaming, end to end
