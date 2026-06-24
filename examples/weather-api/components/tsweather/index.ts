// A TypeScript guest that calls the app's own custom `weather` bridge — proving a TS guest
// reaches a native bridge, like the Rust and Go guests. It runs on a per-app js-runner that
// `rusm build` rebuilt with the bridge's typed host primitives compiled in. `weather` is the
// global that runner exposes, fully typed by the generated `bridges.d.ts` — including the
// **rich types**: `detailed` takes a `query` record and returns a `report` record (with an
// enum), marshaled as JSON, exactly as the Rust and Go guests get native types.
//
// A per-connection WebSocket handler: each text frame is a city; the reply is the forecast.
/// <reference path="../../bridges.d.ts" />
import { websocket } from "rusm-ts";

export default websocket({
  message(socket, data) {
    const city = new TextDecoder().decode(data).trim();
    // String bridge call (the v1 surface) + the rich-typed record/enum round-trip.
    const summary = weather.lookup(city);
    const report = weather.detailed({ city, units: "Celsius" });
    socket.sendText(`${summary} — ${report.sky.toLowerCase()} @ ${report.temp}°C`);
  },
});
