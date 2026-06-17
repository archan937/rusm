// A per-connection WebSocket component in Go (the twin of rs-ws-echo): the host runs
// one process per connection and delivers the writer pid, then each inbound frame; this
// echoes every frame back to the client.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.WebSocket{
		Message: func(c web.Conn, data []byte) {
			c.Send(data) // echo the frame back to the sender
		},
	}.Serve()
}
