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

An SSE handler has three callbacks:

- **`open`** — called once when the connection is established. This is where you
  **subscribe**: `registerTag("todos")` joins this process to the `"todos"` group. Any
  process in the system can broadcast to that tag; every subscriber receives the message
  in its mailbox.
- **`message`** — called each time a message arrives in the mailbox. `stream.data(event)`
  writes the raw bytes as a `data: …\n\n` SSE event on the wire to the client.
- **`close`** — called when the client disconnects. The tag subscription is released
  automatically; this is optional cleanup.

There are no inbound frames from the client — the only input is messages that land in
this process's mailbox, delivered by whoever publishes to the tag.

::: code-group

```ts [TypeScript]
// components/feed/index.ts
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream) {
    // Join the "todos" group: this process will now receive every message
    // that any other process broadcasts to the "todos" tag.
    Process.registerTag("todos");
  },
  message(stream, event) {
    // `event` is the raw bytes of one broadcast message.
    // stream.data() frames it as a `data: …\n\n` SSE event and flushes it to the client.
    stream.data(event);
  },
  close(stream) {
    // Client disconnected. Tag subscription is already released; nothing to do here.
  },
});
```

```rust [Rust]
// components/feed/src/lib.rs
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;
impl Handler for Feed {
    fn open(&mut self, _s: &Stream) {
        // Join the "todos" group: receive every message broadcast to this tag.
        rusm_rs::register_tag("todos");
    }
    fn message(&mut self, s: &Stream, event: Vec<u8>) {
        // `event` is one broadcast payload. Write it as a `data: …` SSE event.
        s.data(&event);
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
		// Join the "todos" group: receive every message broadcast to this tag.
		Open: func(s web.Stream) { rusm.RegisterTag("todos") },
		// ev is one broadcast payload. s.Data() sends it as a `data: …` SSE event.
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },
		Close:   func(s web.Stream) {},
	}.Serve()
}
```

:::

The other half is **publishing**: any process broadcasts to the `"todos"` tag and every
connected feed's `message` fires with the payload. See
[Broadcast to many](/build-an-app/broadcast-to-many). A resident
[service](/build-an-app/build-a-stateful-service) or an HTTP handler is the usual publisher.

## 3. Build, serve, test

```sh
rusm build
rusm serve                                   # feed → http://127.0.0.1:8081
curl -N http://127.0.0.1:8081/               # holds the stream open, printing each data: line
```

`curl -N` connects and waits — with nothing publishing yet you'll see only the periodic
keep-alive, and a `data:` line the moment someone broadcasts to the `todos` tag. Wire up a
publisher next ([Broadcast to many](/build-an-app/broadcast-to-many)) to watch events arrive.

## How it runs

The platform owns the `text/event-stream` head, the wire framing (each `data(...)` becomes a
`data:` event), keep-alive `: ping` heartbeats on idle, and a **bounded, back-pressured** body
— a slow client parks the writer rather than buffering, and a disconnect is detected
immediately (your `close` fires, the subscription auto-releases, the process exits). No poll
loops, no timers: the feed is exactly as live as its publisher.

> SSE can also be **routed** (a `[serve.routes]` table mapping paths to a bare handler) when
> you want per-entity streams on one listener — but a single `component` is the common case.

Next: [Call another component](/build-an-app/call-another-component). For the execution model,
see [the serving model](/deep-dive/the-serving-model) and [byte streams](/deep-dive/byte-streams).
