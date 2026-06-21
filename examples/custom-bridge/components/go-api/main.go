// A Go per-request HTTP handler that calls the app's own custom `weather` bridge — the Go
// twin of components/api (Rust). `rusm build` vendors the bridge WIT, generates the Go
// bindings (internal/wit, from the `bridges` world) and the per-component embedding WIT,
// then TinyGo compiles it. The handler calls the bridge as an ordinary typed Go function.
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"

	forecast "go-api/internal/wit/weather/bridge/forecast"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	h.Handle("forecast", func(_ web.Request, p web.Params) web.Response {
		city := p.Get("city")
		if city == "" {
			city = "nowhere"
		}
		return web.Text(forecast.Lookup(city))
	})
	h.Serve()
}
