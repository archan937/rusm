// A per-request HTTP handler component in Go (the twin of handlers-demo): register
// handler functions by action name and Serve. The host resolves the route and spawns
// this fresh per request; a 3-arg-style HandleSSE handler streams Server-Sent Events.
package main

import (
	"fmt"

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

	h.HandleSSE("ticks", func(_ web.Request, _ web.Params, sse web.Sse) {
		for n := 0; n < 3; n++ {
			if !sse.Data([]byte(fmt.Sprintf("tick %d", n))) {
				break
			}
		}
	})

	// An endless feed: stops only when the client disconnects (back-pressured write
	// returns false). Exercised by the disconnect-teardown guard.
	h.HandleSSE("firehose", func(_ web.Request, _ web.Params, sse web.Sse) {
		for n := 0; ; n++ {
			if !sse.Data([]byte(fmt.Sprintf("ev %d", n))) {
				break
			}
		}
	})

	h.Serve()
}
