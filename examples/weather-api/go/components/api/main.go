// A Go per-request HTTP handler that calls the app's own custom `weather` bridge — the Go
// twin of components/api (Rust). `rusm build` vendors the bridge WIT, generates the Go
// bindings (internal/wit, from the `bridges` world) and the per-component embedding WIT,
// then TinyGo compiles it. The handler calls the bridge as an ordinary typed Go function.
package main

import (
	"fmt"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"

	forecast "api/internal/wit/weather/bridge/forecast"
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
	// The rich-typed bridge call: hand the host a `query` record, get a `report` record (with
	// an enum) back — native Go types from wit-bindgen-go, no marshaling.
	h.Handle("detailed", func(_ web.Request, p web.Params) web.Response {
		city := p.Get("city")
		if city == "" {
			city = "nowhere"
		}
		r := forecast.Detailed(forecast.Query{City: city, Units: forecast.UnitsCelsius})
		sky := "sunny"
		switch r.Sky {
		case forecast.SkyCloudy:
			sky = "cloudy"
		case forecast.SkyRainy:
			sky = "rainy"
		}
		return web.Text(fmt.Sprintf("%s in %s, %d°C", sky, r.City, r.Temp))
	})
	h.Serve()
}
