// A TypeScript guest that calls the app's own custom `weather` bridge — proving a TS guest
// reaches a native bridge, like the Rust and Go guests. It runs on a per-app js-runner that
// `rusm build` rebuilt with the bridge's typed host primitive compiled in; `weather` is the
// global that runner exposes (`globalThis.weather.lookup`, declared below until the generated
// `.d.ts` lands).
//
// A per-connection WebSocket handler: each text frame is a city; the reply is the forecast.
import { websocket } from "rusm-ts";

declare const weather: { lookup(city: string): string };

export default websocket({
  message(socket, data) {
    const city = new TextDecoder().decode(data).trim();
    socket.sendText(weather.lookup(city));
  },
});
