// bridges/weather/host.ts — TypeScript bridge host for the weather example.
// rusm build generates the Rust delegation shim, TS runner, and host binary entry point.
// The runner is a long-lived resident actor; logic here is synchronous — no I/O.

export function lookup(city: string): string {
  return `sunny in ${city}`;
}

// `query` is a WIT record — the generated runner passes it as a plain JS object.
// Return a plain JS object matching the WIT `report` record; enum values are lowercase strings.
export function detailed(query: {
  city: string;
  units: "celsius" | "fahrenheit";
}): { city: string; sky: "sunny" | "cloudy" | "rainy"; temp: number } {
  const temp = query.units === "fahrenheit" ? 70 : 21;
  return { city: query.city, sky: "sunny", temp };
}
