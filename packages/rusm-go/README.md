# rusm-go — the Go guest SDK for RUSM

Write a [RUSM](https://github.com/archan937/rusm) **component** in Go — a sandboxed,
supervised WebAssembly process on an Erlang-style actor runtime. `rusm-go` wraps the
`wit-bindgen-go` actor bindings in a small, idiomatic API: `Pid`, `Send`/`Receive`,
`Spawn`, the registry, process-group tags, `Stream`, `Bucket` (KV), a `Service` with a
typed client, a `Supervisor`, and a `web` subpackage for HTTP/SSE/WebSocket serving. It
is the Go peer of [`rusm-rs`](https://crates.io/crates/rusm-rs) (Rust) and
[`rusm-ts`](https://www.npmjs.com/package/rusm-ts) (TypeScript) — all three share one
JSON wire and interoperate.

Go is **compiled** (TinyGo → a `wasm32-wasip2` component, ~170–490 KB), like Rust — not
interpreted. You write normal Go; `rusm build` runs TinyGo, so your source carries no
bindings boilerplate and no `wit/` dir.

## A component is just your logic

The whole shell is three lines — register the entry in `init`, an empty `main`:

```go
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main()  {}

func run() {
	replyTo, _ := rusm.ParsePid(rusm.ReceiveString())
	rusm.SetLabel("worker")
	rusm.SendBytes(replyTo, []byte("pong from "+rusm.Self().String()))
}
```

Blocking "just works": `Receive` (and `Stream.Read`) suspend the instance on the host
fiber until data arrives — straight-line Go, made async by the host, like an Erlang
`receive`.

## The process API

`rusm.Self/Send/SendBytes/Receive[T]/ReceiveBytes/ReceiveBytesTimeout/Spawn/Monitor/
Register/Whereis/Unregister/SetLabel/List/Info/IsAlive/Kill` plus process-group tags
(`RegisterTag/WhereisTag/KillTag`). `Send` JSON-encodes; the wire is shared with the
Rust and TS guests.

`rusm.SpawnFrom(template, source)` spawns a **dynamic JS** instance of a node-declared
runner template with a runtime-chosen bundle — `inline:<js>` (a plain string),
`kv:<bucket>/<key>`, or `url:`/`http(s)://…`. The loaded JS runs under the template's
declared profile (you choose the code, the operator the capabilities).

## Logging is the standard library

Log the normal way — the `log` package and `log/slog` are routed to the platform logger
(the host stamps the timestamp, `component#pid`, and severity colour; the node's `[log]`
level gates it). No setup, no pid wiring:

```go
slog.Info("served", "path", req.URL, "status", 200)
log.Printf("worker %s started", rusm.Self())
```

## Services and a typed client

A service is a package of handlers with a dispatch loop — register typed handlers with
the `FnN` adapters (no reflection) and run `Serve`:

```go
func run() {
	svc := rusm.NewService()
	svc.Handle("add",   rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
	svc.Handle("greet", rusm.Fn1(func(name string) (string, error) { return "hi " + name, nil }))
	svc.HandleStream("count", func(req rusm.Request, out rusm.Sink) error {
		n, _ := rusm.Arg[int](req, 0)
		for i := 1; i <= n; i++ { out.Send(i) }
		return nil
	})
	svc.Serve()
}
```

Reach it with the generic typed client — blocking `Call`, fire-and-forget `Cast`, or a
range-over-func stream:

```go
sum, _   := rusm.Call[int](pid, "add", 2, 3)               // 5
greet, _ := rusm.Call[string](pid, "greet", "ada")          // "hi ada"
for n := range rusm.CallStream[int](pid, "count", 3) { … }  // 1, 2, 3
```

A **callback** stays in the caller; the service's invocations travel back during the
call (the same `{op:"__cb"}` wire as the other guests):

```go
done, _ := rusm.Call[string](pid, "process", items, rusm.CB(func(p Progress) {
	slog.Info("progress", "done", p.Done)
}))
```

## Supervisor

A struct-literal facade over the host's native supervisor (one restart implementation):

```go
rusm.Supervisor{
	Strategy:    rusm.OneForOne, // or OneForAll, RestForOne
	Children:    []string{"worker", "logger"},
	MaxRestarts: 3,
	Within:      time.Hour, // give up if more than 3 restarts happen in the window
}.Run()
```

## Serving — HTTP / SSE / WebSocket (the `web` subpackage)

Serving is **process-per-unit-of-work**: a fresh instance per HTTP request, one process
per SSE/WS connection — no head-of-line blocking, crash containment per unit. HTTP routing
is declarative in `rusm.toml` (`"METHOD /path/:param" = "component#action"`).

An HTTP component — buffered, routed actions:

```go
import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main()  {}

func run() {
	h := web.NewHandlers()
	h.Handle("home", func(req web.Request, p web.Params) web.Response {
		return web.Text("hi " + p.Get("name"))
	})
	h.Serve()
}
```

An SSE component — one process per connection (like WebSocket): `Open` subscribes to an
event source (e.g. a process-group tag) and emits initial events; `Message` emits each
event pushed to the mailbox:

```go
func run() {
	web.Sse{
		Open:    func(s web.Stream) { rusm.RegisterTag("ticks") }, // subscribe
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },     // a pushed event → emit
		Close:   func(s web.Stream) {},                            // disconnect — clean or dropped
	}.Serve()
}
```

A WebSocket component (one process per connection):

```go
func run() {
	web.WebSocket{
		Open:    func(c web.Conn) { c.Send([]byte("welcome")) },
		Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo this connection's frame
		Close:   func(c web.Conn) {},                            // disconnect — clean or dropped
	}.Serve()
}
```

## Streams and KV

```go
s, ok := rusm.OpenStream(pid)          // back-pressured byte stream
s.Write(chunk); s.Close()
b := rusm.OpenBucket("specs")          // durable KV (storage capability)
b.Set("k", []byte("v")); v, ok, _ := b.Get("k")
```

## Build

The fastest start is `rusm new <name> --lang go` (scaffolds a component + `rusm.toml`).
`rusm build` compiles each `components/<name>/` with a `go.mod` via TinyGo:

```
tinygo build -target=wasip2 -no-debug -panic=trap -opt=z \
  -wit-package <rusm-go>/wit -wit-world component -o wasm/<name>.wasm .
```

A component module just requires this SDK:

```
require github.com/archan937/rusm/packages/rusm-go v0.4.2
```

Toolchain: Go + TinyGo (0.41+) + `wit-bindgen-go` + `wasm-tools` + `binaryen` (wasm-opt).
`-panic=trap` makes a Go panic a wasm trap → the process is `Crashed` (RUSM's crash
model), and supervision restarts it.
