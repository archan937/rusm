// A per-connection SSE handler in TypeScript that emits a rich event (data + id + event)
// via stream.emit, then closes — the TS twin of rs/go-sse-event, proving rusm-ts's emit
// reaches the client through the js-runner's __sse_send primitive. Built with:
//   bun build --target=browser --format=cjs --outfile ts_sse_event.js index.ts
import { sse } from "../../../../../packages/rusm-ts/index";

export default sse({
  open: (s) => {
    s.emit({ data: "hello", id: "42", event: "greeting" });
    s.close();
  },
  message: () => {},
});
