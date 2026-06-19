// A per-connection SSE handler in TypeScript that reports its connection context (method,
// path, query, a captured route param, a header) to the registered `collector` on open —
// the TS twin of rs-sse-conn / go-sse-conn, proving rusm-ts's `sse().info` reaches a real
// TS handler through the js-runner's `__connection` primitive. Built with:
//   bun build --target=browser --format=cjs --outfile ts_sse_conn.js index.ts
import { sse, Process } from "../../../../../packages/rusm-ts/index";

export default sse({
  open: (s) => {
    const i = s.info;
    const report = `${i.method} ${i.path} q=${i.query} plan=${i.param("plan") ?? "-"} host=${i.header("host") ?? "?"}`;
    const collector = Process.whereis("collector");
    if (collector !== null) Process.send(collector, report);
  },
  // SSE is server→client only; no inbound events for this context probe.
  message: () => {},
});
