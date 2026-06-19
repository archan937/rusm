// A per-connection SSE handler in Go that emits a rich event (data + id + event name) via
// Stream.Emit, then closes — the Go twin of rs-sse-event, proving the additive sse-send op.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open: func(s web.Stream) {
			s.Emit(web.Event{Data: []byte("hello"), ID: "42", Name: "greeting"})
			s.Close()
		},
		Message: func(s web.Stream, _ []byte) {},
	}.Serve()
}
