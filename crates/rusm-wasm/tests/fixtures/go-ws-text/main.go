// A WebSocket handler in Go that replies with a TEXT frame via Conn.SendText — the Go twin
// of rs-ws-text, proving the additive ws-send-text op (the default Send is binary).
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.WebSocket{
		Message: func(c web.Conn, data []byte) { c.SendText(string(data)) },
	}.Serve()
}
