# Serve WebSocket

A WebSocket server in RUSM is **one sandboxed process per connection**. The host owns the
socket; each inbound frame arrives as a message, and you reply with `send`. We'll build an
echo server, then run it.

## 1. Declare the listener

WebSocket (like SSE) isn't routed — it runs **one handler component per connection**, so you
name it directly with `component = "..."`:

```toml
[[serve]]
protocol = "ws"
component = "chat"            # one process per connection → ./wasm/chat.{wasm,js}
listen = "127.0.0.1:8082"

[components.chat]
capability = "sandboxed"
```

## 2. Write the handler

Unlike SSE, WebSocket is **bidirectional**: the client sends frames, the server receives
them and can reply. This shows up directly in the handler shape.

**`socket` / `conn`** is the handle to *this* client's connection — the thing you call
`.send()` on to push a frame back to that specific browser. It is not a shared channel;
every connection has its own.

**`data`** in `message` is the raw payload of one inbound frame from the client. You
decide what to do with it: echo it, parse it as a command, forward it to another process,
ignore it — that's your handler logic.

Three callbacks — `open`, `message`, `close`; only `message` is required:

- **`open(socket)`** — the client connected. Use it to send a greeting, register the
  connection with a service, or join a broadcast group. `socket.send()` pushes a frame
  immediately.
- **`message(socket, data)`** — the client sent a frame. `data` is its raw bytes.
  `socket.send()` sends a frame back to that client. This is the core of the
  request/reply loop — or broadcast fan-out, or whatever your protocol does.
- **`close(socket)`** — the client disconnected (clean close or dropped socket). The
  process is about to exit; unregister from any groups or services here.

There is **one handler instance per connection** — its state (a Rust `&mut self`, a TS
closure, a Go local variable) is private to that connection. Nothing is shared with other
clients.

::: code-group

```ts [TypeScript]
// components/chat/index.ts
import { websocket } from "rusm-ts";

export default websocket({
  open(socket) {
    // Client connected. Push a greeting frame back to this client.
    socket.send("welcome\n");
  },
  message(socket, data) {
    // Client sent a frame. `data` is its raw bytes.
    // socket.send() pushes a frame back to this same client — here, a plain echo.
    socket.send(data);
  },
  close(socket) {
    // Client disconnected (clean or dropped). Process exits after this.
  },
});
```

```rust [Rust]
// components/chat/src/lib.rs
use rusm_rs::ws::{self, Connection, Handler};

struct Echo;
impl Handler for Echo {
    fn open(&mut self, conn: &Connection) {
        // Push a greeting frame to this client.
        conn.send(b"welcome\n");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        // `data` is one inbound frame from the client. Echo it straight back.
        conn.send(&data);
    }
    fn close(&mut self, _conn: &Connection) {}
}

#[rusm_rs::main]
fn run() {
    ws::serve(Echo);
}
```

```go [Go]
// components/chat/main.go
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	web.WebSocket{
		// Push a greeting frame to this client.
		Open: func(c web.Conn) { c.Send([]byte("welcome\n")) },
		// `data` is one inbound frame from the client. Echo it straight back.
		Message: func(c web.Conn, data []byte) { c.Send(data) },
		Close:   func(c web.Conn) {},
	}.Serve()
}
```

:::

## 3. Build, serve, test

```sh
rusm build
rusm serve   # chat → ws://127.0.0.1:8082
# in another shell (Bun's WebSocket):
bun -e 'const w=new WebSocket("ws://127.0.0.1:8082");w.onmessage=e=>console.log(""+e.data);w.onopen=()=>w.send("hi")'
# → welcome
# → hi
```

## How it runs

Each connection is a **fresh sandboxed process**; when the client disconnects (clean close or
a dropped socket) your `close` fires once and the process exits — the runtime reclaims
everything it held. A crash in one connection's handler drops *that* connection only; every
other client and the listener are untouched.

**Talking to many clients at once** (a chat room fanning one message to its members) doesn't
go through shared state — each connection tags itself with a **process-group tag** and a
publisher broadcasts to the tag. That's its own pattern: [Broadcast to many](/build-an-app/broadcast-to-many).
Cross-connection state (presence counts, history) belongs in a
[stateful service](/build-an-app/build-a-stateful-service) or `kv`, never in the per-connection process.

Next: [Serve SSE](/build-an-app/serve-sse). For the execution model + failure modes, see
[the serving model](/deep-dive/the-serving-model).
