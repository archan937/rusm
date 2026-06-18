# Lifecycle — SSE component

A per-request **Server-Sent Events** handler: a 3-arg action `fn(Request, Params, Sse)`
that streams a `text/event-stream` body for the life of one connection. Because each
request is its own process, the action may block for the whole connection. See the
[overview](./component-lifecycle.md) for the shared two-domain model and failure
vocabulary.

## Shape (what you write)

::: code-group

```rust [Rust]
use rusm_rs::http::{Params, Request, Sse};

#[rusm_rs::handlers]
pub mod api {
    use super::*;
    // routed from `[serve.routes]`:  "GET /feed" = "api#feed"
    pub fn feed(_req: Request, _p: Params, sse: Sse) {
        for n in 0.. {
            if !sse.data(format!("tick {n}").as_bytes()) {
                break; // the client disconnected — stop cleanly
            }
        }
    }
}
```

```ts [TypeScript]
// A wasi:http component returning a streaming text/event-stream body. The runtime
// pulls events and applies back-pressure; closing the controller ends the stream.
export default function handle(_request: Request): Response {
  const enc = new TextEncoder();
  let n = 0;
  const body = new ReadableStream({
    pull(controller) {
      controller.enqueue(enc.encode(`data: tick ${n++}\n\n`));
    },
  });
  return new Response(body, { headers: { "content-type": "text/event-stream" } });
}
```

```go [Go]
package main

import (
	"fmt"

	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	h := web.NewHandlers()
	// routed from `[serve.routes]`:  "GET /feed" = "api#feed"
	h.HandleSSE("feed", func(_ web.Request, _ web.Params, sse web.Sse) {
		for n := 0; ; n++ {
			if !sse.Data([]byte(fmt.Sprintf("tick %d", n))) {
				break // the client disconnected — stop cleanly
			}
		}
	})
	h.Serve()
}
```

:::

In Rust, `sse.data(bytes)` writes one `data:` event and `sse.run(heartbeat_ms, map)`
live-tails a source (e.g. a pub/sub topic) with idle heartbeats — the loop ends when a
write returns `false`. Go's `web.Sse` mirrors this — `sse.Data(bytes)` and
`sse.Run(heartbeatMs, mapFn)`. In TypeScript the runtime drives the `ReadableStream`'s
`pull`, parking it on back-pressure and ending it on disconnect — the same lifecycle.

## Platform owns / you write

- **Platform owns:** the **bounded, back-pressured** byte stream from the guest into the
  chunked HTTP body, disconnect detection (a failed write returns `false`), reclaim on
  exit, and **SSE transport correctness** — `Cache-Control: no-cache` is added for you,
  and on a `protocol = "sse"` listener a reply that isn't `text/event-stream` fails loud
  (500) instead of silently serving a broken stream. The Rust and Go SDKs set the
  `text/event-stream` head for you; a hand-written TS `fetch` sets it itself, so that
  contract check is what catches a forgotten header.
- **You write:** the event loop — produce events, react to a `false` write by stopping.

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Normal** (finite feed) | drains each event into the body; closes the body when you return | writes events, then returns | clean end-of-stream |
| **Back-pressure** (consumer slow) | the bounded channel fills; the next write **parks the fiber** until drained | blocked inside `sse.data(…)`; resumes later | paced to the consumer — **never a busy-spin** |
| **Client disconnect** | the stream reader drops, so the next write returns `false` | the loop `break`s; the process exits | prompt teardown; a broker that `monitor`ed it prunes on the `Down` |
| **Connection error** (socket write fails mid-stream) | same as a disconnect — the write returns `false` | loop breaks | clean exit |
| **Crash (trap)** mid-stream | the stream writer drops → the chunked body simply ends → the process is Crashed | the `panic!` / `.unwrap()` | a truncated stream; **only this connection** |
| **Memory crash (OOM)** | the `StoreLimiter` cap trips a trap → body ends → Crashed | exceeded `max-memory-mb` | truncated stream; the instance is discarded |

## Notes

- **No spins, ever.** The byte stream is a bounded channel, so an *endless* feed is
  paced by the consumer (the write parks the fiber) — it cannot peg a core. And a
  disconnect always tears the process down. Both are **regression-guarded by a test**
  (an endless feed + a forced disconnect, asserting the process count returns to
  baseline).
- **Live fan-out pattern.** Subscribe to a [service component](./lifecycle-service.md)
  (a broker) and `sse.run(...)` to forward each published message. The service
  `monitor`s subscribers, so this process's exit (on disconnect) prunes the
  subscription automatically — crash-safe cleanup with no unsubscribe call.

Prev: [HTTP component](./lifecycle-http.md) · Next: [WebSocket component](./lifecycle-websocket.md)
