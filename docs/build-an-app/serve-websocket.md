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

Three optional callbacks — `open`, `message`, `close`; only `message` is required. There's
**one handler instance per connection**, so its state (a Rust `&mut self`, a TS/Go closure) is
*this* connection's — nothing is shared with other clients.

::: code-group

```ts [TypeScript]
// components/chat/index.ts
import { websocket } from "rusm-ts";

export default websocket({
  open(socket) {
    socket.send("welcome\n");
  },
  message(socket, data) {
    socket.send(data); // echo this connection's frame
  },
  close(socket) {
    // disconnect — clean or dropped (optional)
  },
});
```

```rust [Rust]
// components/chat/src/lib.rs
use rusm_rs::ws::{self, Connection, Handler};

struct Echo;
impl Handler for Echo {
    fn open(&mut self, conn: &Connection) {
        conn.send(b"welcome\n");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        conn.send(&data); // echo this connection's frame
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
		Open:    func(c web.Conn) { c.Send([]byte("welcome\n")) },
		Message: func(c web.Conn, data []byte) { c.Send(data) }, // echo
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
publisher broadcasts to the tag. That's its own pattern: [Broadcast to many](/build-an-app/broadcast).
Cross-connection state (presence counts, history) belongs in a
[stateful service](/build-an-app/stateful-service) or `kv`, never in the per-connection process.

Next: [Serve SSE](/build-an-app/serve-sse). For the execution model + failure modes, see
[the serving model](/deep-dive/serving-model).
