// A per-connection SSE handler in Go (the twin of rs-sse-feed / go-ws-lifecycle): open
// subscribes to the "feed" process-group tag and reports "open", message emits each
// pushed event, close reports "close" — so a test can assert push-via-tags and that close
// fires on disconnect.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func report(event string) {
	if collector, ok := rusm.Whereis("collector"); ok {
		rusm.SendBytes(collector, []byte(event))
	}
}

func run() {
	web.Sse{
		Open:    func(s web.Stream) { rusm.RegisterTag("feed"); report("open") }, // subscribe
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },                    // emit the pushed event
		Close:   func(s web.Stream) { report("close") },
	}.Serve()
}
