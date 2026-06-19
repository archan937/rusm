// A per-connection SSE handler in Go that reports its connection context (method, path,
// query, a captured route param, a header) to the registered collector on open — the Go
// twin of rs-sse-conn, proving the `connection` op reaches a Go per-connection handler.
package main

import (
	"fmt"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open: func(s web.Stream) {
			i := s.Info()
			plan := i.Param("plan")
			if plan == "" {
				plan = "-"
			}
			host := i.Header("host")
			if host == "" {
				host = "?"
			}
			report := fmt.Sprintf("%s %s q=%s plan=%s host=%s", i.Method(), i.Path(), i.Query(), plan, host)
			if collector, ok := rusm.Whereis("collector"); ok {
				rusm.SendBytes(collector, []byte(report))
			}
		},
		// SSE is server→client only; no inbound events for this context probe.
		Message: func(s web.Stream, _ []byte) {},
	}.Serve()
}
