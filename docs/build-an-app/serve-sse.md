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

Two things to be clear about before looking at the code:

**`stream` is the outbound SSE connection to the browser** — the HTTP response body you
write `data:` events into. It has nothing to do with the actor message stream; it's just
the open pipe to the client.

**SSE is strictly server→client.** The browser never sends messages back over this
connection — there are no inbound frames. The only input this process ever receives is
messages from the actor system: other processes that broadcast to a tag this process
joined. That is what the `message` callback is for.

The three callbacks:

- **`open`** — the client connected. `registerTag("todos")` joins this process to the
  `"todos"` group. From this point on, every broadcast to that tag lands as a message in
  this process's mailbox.
- **`message(stream, event)`** — a message arrived in the mailbox. `event` is the raw
  payload one broadcaster sent. `stream.data(event)` frames it as a `data: …\n\n` SSE
  event and flushes it down the wire to the browser.
- **`close`** — the client disconnected. The tag subscription is released automatically;
  this is optional cleanup.

In this single-tag example there's nothing to route — every mailbox message is a `todos`
broadcast and every one goes straight to the client. If you joined **multiple tags** (say
`"todos"` and `"alerts"`) you'd inspect `event` to decide how to frame or filter each
message — that's where an `if` or a `switch` on the payload type would appear.

::: code-group

```ts [TypeScript]
// components/feed/index.ts
import { sse, Process } from "rusm-ts";

export default sse({
  open(stream) {
    // Join the "todos" broadcast group. From now on, every Process.publishTag("todos", …)
    // call from anywhere in the system delivers a message to this process's mailbox.
    Process.registerTag("todos");
  },
  message(stream, event) {
    // A broadcast landed in the mailbox. `event` is its raw payload (bytes).
    // stream.data() frames it as a `data: …\n\n` SSE event on the wire to the browser.
    stream.data(event);
  },
  close(stream) {
    // Browser disconnected. Tag membership is released automatically.
  },
});
```

```rust [Rust]
// components/feed/src/lib.rs
use rusm_rs::sse::{self, Handler, Stream};

struct Feed;
impl Handler for Feed {
    fn open(&mut self, _s: &Stream) {
        // Join the "todos" broadcast group.
        rusm_rs::register_tag("todos");
    }
    fn message(&mut self, s: &Stream, event: Vec<u8>) {
        // A broadcast arrived in the mailbox. Forward it as a `data: …` SSE event.
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
		// Join the "todos" broadcast group.
		Open: func(s web.Stream) { rusm.RegisterTag("todos") },
		// A broadcast arrived in the mailbox. Forward it as a `data: …` SSE event.
		Message: func(s web.Stream, ev []byte) { s.Data(ev) },
		Close:   func(s web.Stream) {},
	}.Serve()
}
```

:::

The other half is **publishing**: any process broadcasts to `"todos"` and every connected
feed's `message` fires with the payload — see
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
