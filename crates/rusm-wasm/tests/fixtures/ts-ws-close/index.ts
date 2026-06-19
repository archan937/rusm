// A WebSocket handler in TypeScript that closes with a status code + reason via
// socket.close — the TS twin of rs/go-ws-close, proving rusm-ts's close reaches the client
// through the js-runner's __ws_close primitive. Built with:
//   bun build --target=browser --format=cjs --outfile ts_ws_close.js index.ts
import { websocket } from "../../../../../packages/rusm-ts/index";

export default websocket({
  message: (s) => {
    s.close(1000, "bye");
  },
});
