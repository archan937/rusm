// A WebSocket handler in TypeScript that replies with a TEXT frame via socket.sendText —
// the TS twin of rs-ws-text / go-ws-text, proving rusm-ts's sendText reaches the client as
// a text frame through the js-runner's __ws_send_text primitive. Built with:
//   bun build --target=browser --format=cjs --outfile ts_ws_text.js index.ts
import { websocket } from "../../../../../packages/rusm-ts/index";

export default websocket({
  message: (s, data) => {
    s.sendText(new TextDecoder().decode(data));
  },
});
