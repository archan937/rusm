# Lifecycle — SSE component

One sandboxed component process **per connection** — the SSE twin of the
[WebSocket component](./lifecycle-websocket.md). SSE is one-way (server → client): the
handler emits events; there are no inbound client frames. The host owns the response body
and delivers events to the handler through its **mailbox** — typically a
[process-group tag](./links-and-supervision.md) the handler subscribes to, so a
publisher's broadcast fans out to every open stream (push, not polling). See the
[overview](./component-lifecycle.md) for the shared two-domain model.

## Shape (what you write)

*The same shape, wired into a real app, is the `feed` component of the todo-board examples — [TypeScript](https://github.com/archan937/rusm/tree/main/examples/typescript/components/feed) · [Rust](https://github.com/archan937/rusm/tree/main/examples/rust/components/feed) · [Go](https://github.com/archan937/rusm/tree/main/examples/go/components/feed).*

::: code-group

```ts [TypeScript]
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream) {
    Process.registerTag("todos"); // subscribe to the event source
  },
  message(stream, event) {
    stream.data(event); // a published event → emit it
  },
  close(stream) {
    // disconnect — clean or dropped (optional)
  },
});
```

```rust [Rust]
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;
impl Handler for Feed {
    fn open(&mut self, _s: &Stream) {
        rusm_rs::register_tag("todos"); // subscribe to the event source
    }
    fn message(&mut self, s: &Stream, event: Vec<u8>) {
        s.data(&event); // a published event → emit it
    }
    fn close(&mut self, _s: &Stream) {
        // disconnect — clean or dropped (optional)
    }
}

#[rusm_rs::main]
fn run() {
    sse::serve(Feed);
}
```

```go [Go]
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open:    func(s web.Stream) { rusm.RegisterTag("todos") },   // subscribe
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },       // a published event → emit
		Close:   func(s web.Stream) {},                              // disconnect
	}.Serve()
}
```

:::

One handler instance per connection, so its state is *this connection's*. `open` and
`close` are optional; only `message` is required. **Publishing** is the other half: any
process broadcasts to the tag — `whereis_tag("todos")` then `send` each pid (the
[pub/sub primitive](./links-and-supervision.md)) — and every open stream's `message`
fires. A resident `[components.<name>]` service or an HTTP handler is the usual publisher.

## Platform owns / you write

- **Platform owns:** the `text/event-stream` head + `Cache-Control: no-cache`, the SSE
  wire framing (each `data(...)` payload becomes a `data:` event), keep-alive `: ping`
  heartbeats on idle, the **bounded, back-pressured** body, disconnect detection (the
  writer process that owns the body dies → the handler, monitoring it, runs `close`), and
  reclaim on exit (subscriptions auto-release).
- **You write:** `open` (subscribe), `message` (emit each event with `stream.data`), and
  optionally `close` (cleanup). Self-stop with `stream.close()`.

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Open** | spawn → deliver msg 1 = writer pid → send the `text/event-stream` head | `open` subscribes (e.g. `register_tag`) | stream live, awaiting events |
| **Event pushed** | a publisher's broadcast lands in the mailbox | `message` emits it via `stream.data` | one `data:` event flushed |
| **Idle** | no event for the heartbeat window → the writer emits `: ping` | — | connection kept alive through proxies |
| **Client disconnect** (close or dropped) | the body's reader drops → the writer dies → the monitored death surfaces | `close` fires once, then the process exits | socket closed; subscription released |
| **Self-stop** | `stream.close()` ends the body | `close` fires; the handler returns | clean end-of-stream; the client sees EOF |
| **Crash (trap)** | the process is Crashed; the writer + body torn down | the `panic!` / `.unwrap()` | a truncated stream; **only this connection** |

## Notes

- **No spins, ever.** The body is a bounded channel: a slow consumer back-pressures the
  writer (it parks), and a disconnect is detected immediately (the body channel's
  `closed()` resolves), not only at the next heartbeat — so an idle or endless feed costs
  ~nothing and never leaks. Regression-guarded by the disconnect-teardown test.
- **Push, not polling.** Events arrive through the mailbox, so there's no poll loop and no
  timer — the feed is exactly as live as its publisher. The same process-group tags power
  [WebSocket](./lifecycle-websocket.md) broadcast and any 1→N fan-out.
- **Shared state lives elsewhere.** Cross-connection state belongs in a
  [service component](./lifecycle-service.md) or `kv`, never in the per-connection process.

Prev: [HTTP component](./lifecycle-http.md) · Next: [WebSocket component](./lifecycle-websocket.md)
