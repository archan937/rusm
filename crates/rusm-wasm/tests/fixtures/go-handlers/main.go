// A per-request HTTP handler component in Go (the twin of handlers-demo): register
// buffered handler functions by action name and Serve. The host resolves the route and
// spawns this fresh per request. (SSE is a per-connection handler now — see web.Sse.)
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()

	h.Handle("hello", func(_ web.Request, p web.Params) web.Response {
		name := p.Get("name")
		if name == "" {
			name = "world"
		}
		return web.Text("hi " + name + "\n")
	})

	h.Handle("echo", func(req web.Request, _ web.Params) web.Response {
		return web.Bytes(200, req.Body)
	})

	h.Serve()
}
