# Serve SSE

Server-Sent Events (SSE) is a one-way live stream: the server pushes `data:` events, the
browser receives them over a plain `EventSource`. Like WebSocket, RUSM runs **one process per
connection** — but it's push-only (no inbound frames). The handler **subscribes** to an event
source and **emits** each event it receives. We'll build a live feed.

## 1. Declare the listener

SSE runs one handler per connection, named directly with `component = "..."`:

```toml
[[serve]]
protocol = "sse"
component = "feed"           # one process per connection → ./wasm/feed.{wasm,js}
listen = "127.0.0.1:8081"

[components.feed]
capability = "sandboxed"
```

## 2. Write the handler

`open` subscribes (here to a **process-group tag** — the pub/sub primitive), `message` emits
each event with `data(...)`, `close` is optional cleanup. There are no inbound frames — events
arrive through the mailbox from whoever publishes to the tag.

::: code-group

```ts [TypeScript]
// components/feed/index.ts
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream) {
    Process.registerTag("todos"); // subscribe to the event source
  },
  message(stream, event) {
    stream.data(event); // a published event → emit it as `data: …`
  },
  close(stream) {},
});
```

```rust [Rust]
// components/feed/src/lib.rs
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;
impl Handler for Feed {
    fn open(&mut self, _s: &Stream) {
        rusm_rs::register_tag("todos"); // subscribe
    }
    fn message(&mut self, s: &Stream, event: Vec<u8>) {
        s.data(&event); // a published event → emit
    }
    fn close(&mut self, _s: &Stream) {}
}

#[rusm_rs::main]
fn run() {
    sse::serve(Feed);
}
```

```go [Go]
// components/feed/main.go
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.Sse{
		Open:    func(s web.Stream) { rusm.RegisterTag("todos") },  // subscribe
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },      // emit
		Close:   func(s web.Stream) {},
	}.Serve()
}
```

:::

The other half is **publishing**: any process broadcasts to the `todos` tag and every open
stream's `message` fires — see [Broadcast to many](/build-an-app/broadcast). A resident
[service](/build-an-app/stateful-service) or an HTTP handler is the usual publisher.

## 3. Build, serve, test

```sh
rusm build
rusm serve                                   # feed → http://127.0.0.1:8081
curl -N http://127.0.0.1:8081/               # streams data: lines as they're published
```

## How it runs

The platform owns the `text/event-stream` head, the wire framing (each `data(...)` becomes a
`data:` event), keep-alive `: ping` heartbeats on idle, and a **bounded, back-pressured** body
— a slow client parks the writer rather than buffering, and a disconnect is detected
immediately (your `close` fires, the subscription auto-releases, the process exits). No poll
loops, no timers: the feed is exactly as live as its publisher.

> SSE can also be **routed** (a `[serve.routes]` table mapping paths to a bare handler) when
> you want per-entity streams on one listener — but a single `component` is the common case.

Next: [Call another component](/build-an-app/call-another-component). For the execution model,
see [the serving model](/deep-dive/serving-model) and [byte streams](/deep-dive/byte-streams).
