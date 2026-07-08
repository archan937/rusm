// bridges/weather/host.go — Go bridge host for the weather example.
// rusm build generates _runner.go and go.mod (if absent); TinyGo compiles the whole
// bridges/weather/ package to wasm/bridge-weather.wasm. Logic here is synchronous.
package main

import (
	"encoding/json"
	"fmt"
)

// Lookup returns a forecast string for the city.
func Lookup(city string) string {
	return fmt.Sprintf("sunny in %s", city)
}

// Detailed returns a structured forecast.
// WIT record params arrive as json.RawMessage in the generated dispatcher — unmarshal into your
// own struct for full control over field naming and validation. WIT `enum` values on the wire
// are the variant names as the bindings render them (`Units` = "Celsius"/"Fahrenheit", `Sky` =
// "Sunny"/"Cloudy"/"Rainy") — a bridge host must read/write those exact spellings.
func Detailed(raw json.RawMessage) json.RawMessage {
	var q struct {
		City  string `json:"city"`
		Units string `json:"units"`
	}
	if err := json.Unmarshal(raw, &q); err != nil {
		q.City = "unknown"
		q.Units = "Celsius"
	}
	temp := int32(21)
	if q.Units == "Fahrenheit" {
		temp = 70
	}
	out, _ := json.Marshal(map[string]any{
		"city": q.City,
		"sky":  "Sunny",
		"temp": temp,
	})
	return out
}
