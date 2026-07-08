// bridges/weather/host.ts — TypeScript bridge host for the weather example.
// `rusm build` generates the Rust delegation shim, the TS runner, and the host binary. The
// runner is a long-lived resident actor; logic here is synchronous — no I/O.
//
// WIT `enum` values on the wire are the variant names as the generated bindings render them —
// see the ambient `bridges.d.ts` this build writes (`Units = "Celsius" | "Fahrenheit"`,
// `Sky = "Sunny" | "Cloudy" | "Rainy"`). A guest is type-checked against those; a bridge host
// must return/accept the same spellings.

export function lookup(city: string): string {
  return `sunny in ${city}`;
}

// `query` is a WIT `record` (a plain JS object); the return matches the WIT `report` record,
// with the `sky` enum as one of the generated `Sky` values.
export function detailed(query: {
  city: string;
  units: "Celsius" | "Fahrenheit";
}): { city: string; sky: "Sunny" | "Cloudy" | "Rainy"; temp: number } {
  const temp = query.units === "Fahrenheit" ? 70 : 21;
  return { city: query.city, sky: "Sunny", temp };
}
