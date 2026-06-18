// A per-connection WebSocket handler in Go (the twin of rs-ws-lifecycle): exercises
// the full lifecycle — open / message (echo) / close — reporting "open"/"close" to a
// registered `collector` so a test can assert `Close` fires when the socket drops.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main()  {}

func report(event string) {
	if collector, ok := rusm.Whereis("collector"); ok {
		rusm.SendBytes(collector, []byte(event))
	}
}

func run() {
	web.WebSocket{
		Open:    func(c web.Conn) { report("open") },
		Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo
		Close:   func(c web.Conn) { report("close") },
	}.Serve()
}
