# Write a Go component

A Go guest is **idiomatic Go**, compiled with **TinyGo** to `wasm32-wasip2` and run as a
first-class sandboxed RUSM process — the same capabilities, memory cap, and epoch preemption
as a Rust or TypeScript component. `rusm new --lang go` scaffolds one; `rusm build` drives
TinyGo for you (no hand-rolled bindings). The actor API and the typed client come from the
**`rusm-go`** package.

A Go component lives under `components/<name>/` with a `main.go`. There are two shapes — a
**worker** (runs once) and a **service** (a dispatch loop) — mirroring the TypeScript and
Rust models over the same JSON wire, so a Go client and a TS or Rust service interoperate.

## A worker

`rusm.Run` registers your entry; `main` stays empty (the runtime drives it):

```go
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	rusm.Register("worker")          // name yourself in the registry
	msg := rusm.ReceiveBytes()       // block for a message (the fiber parks)
	rusm.SendBytes(rusm.Self(), msg) // echo to self, etc.
}
```

The `Process` API is the Erlang toolkit — `Self` / `Send` / `Receive` / `Spawn` /
`Register` / `Whereis` / `IsAlive` / `Kill` / `SetLabel` / `RegisterTag` / `WhereisTag`
(process groups) / `OpenStream` / `AcceptStream` — see
[process management](/build-an-app/coordinate-and-supervise).

## A service + typed client

A **service** registers typed handlers; the runtime runs the receive → dispatch → reply
loop. A caller reaches it with the generic `Call[R]` client — a real cross-process message,
hidden behind a function call:

```go
func init() { rusm.Run(run) }
func main() {}

func run() {
	svc := rusm.NewService()
	svc.Handle("add", rusm.Fn2(func(a, b int) (int, error) { return a + b, nil }))
	svc.HandleStream("countTo", func(req rusm.Request, out rusm.Sink) error { // streaming
		n, _ := rusm.Arg[int](req, 0)
		for i := 1; i <= n; i++ {
			out.Send(i)
		}
		return nil
	})
	svc.Serve()
}

// caller (another component):
//   calc, _ := rusm.Spawn("calc")
//   sum, _ := rusm.Call[int](calc, "add", 2, 3)
```

Declare both in `rusm.toml` under `[components.<name>]`, exactly like a Rust or TS component
(the spawner needs the `allow-spawn` capability). Errors are ordinary Go `error`s; logging is
the standard `log` / `log/slog` packages, routed to the node's log stream by the SDK — no
setup, no name/pid wiring (the host stamps them).

## Build & run

```sh
rusm build   # TinyGo: components/*/main.go → wasm32-wasip2 → ./wasm/<name>.wasm
rusm run     # spawn them per rusm.toml
rusm dev     # build + run, then watch ./components and reload on edit
```

`rusm build` runs TinyGo for each Go component and generates the WIT bindings it needs;
you write plain Go. To serve a Go component over HTTP/WS/SSE, see
[Serve over HTTP, WebSocket & SSE](/build-an-app/serve-http). The runnable
[`go`](https://github.com/archan937/rusm/tree/main/examples/go) todo-board example is the
same model wired end to end.
