// A WebSocket handler in Go that closes with a status code + reason via Conn.Close — the Go
// twin of rs-ws-close, proving the additive ws-close op.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.WebSocket{
		Message: func(c web.Conn, _ []byte) { c.Close(1000, "bye") },
	}.Serve()
}
