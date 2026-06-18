# Lifecycle — WebSocket component

One sandboxed component process **per connection**. The host owns the socket and
delivers each inbound frame to the process's mailbox; the process replies through a
writer pid. See the [overview](./component-lifecycle.md) for the shared two-domain
model and failure vocabulary.

## Shape (what you write)

::: code-group

```rust [Rust]
use rusm_rs::ws::{self, Connection, Handler};

struct Echo;
impl Handler for Echo {
    fn open(&mut self, conn: &Connection) {
        conn.send(b"welcome\n");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        conn.send(&data); // echo this connection's frame
    }
    fn close(&mut self, _conn: &Connection) {
        // disconnect — clean or dropped (optional)
    }
}

#[rusm_rs::main]
fn run() {
    ws::serve(Echo);
}
```

```ts [TypeScript]
import { websocket } from "rusm-ts";

// One worker per connection; reply with `socket.send(…)`.
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

```go [Go]
package main

import (
	rusm "github.com/archan937/rusm/packages/rusm-go"
	"github.com/archan937/rusm/packages/rusm-go/web"
)

func init() { rusm.Run(run) }
func main() {}

func run() {
	// One process per connection; reply with c.Send(…).
	web.WebSocket{
		Open: func(c web.Conn) {
			c.Send([]byte("welcome\n"))
		},
		Message: func(c web.Conn, data []byte) {
			c.Send(data) // echo this connection's frame
		},
		Close: func(c web.Conn) {
			// disconnect — clean or dropped (optional)
		},
	}.Serve()
}
```

:::

There is **one handler instance per connection**, so its state (Rust `&mut self`, a
TypeScript closure, or a Go closure) is *this connection's* state — no cross-connection
sharing. `open` and `close` are optional; only `message` is required.

## Platform owns / you write

- **Platform owns:** the upgrade handshake, the socket and its **writer process** (a
  Wasm-free process that owns the sink — message 1 to your process is its pid), and
  delivering inbound frames as mailbox messages. The writer process's death **is** the
  disconnect (clean close or a dropped socket alike); the SDK monitors it and turns it
  into your `close` callback, then the per-connection process exits.
- **You write:** `open` / `message` / `close`, replying with `conn.send(…)`.

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Normal** open + frames | upgrade → spawn → deliver msg 1 = writer pid → each frame as a message | `open`, then `message` per frame, replying via `conn.send` | frames handled/echoed |
| **Client disconnect** (clean close or dropped socket) | the writer process dies; the monitored death surfaces to the handler | `close` fires once, then the process exits | socket closed; resources reclaimed |
| **Connection error** (reset, bad frame, protocol error) | the connection task ends; the writer dies | `close` fires (the disconnect path) | that connection gone |
| **Crash (trap)** in a handler | the process is Crashed; the platform tears down its writer + socket | the `panic!` / `.unwrap()` (no `close` — the handler is already dead) | that connection dropped; **all others + the listener untouched** |
| **Memory crash (OOM)** | the `StoreLimiter` cap trips a trap → handled like a crash | exceeded `max-memory-mb` | that connection dropped; the instance discarded |

## Notes

- **Containment by construction.** Connections share nothing, so a crash or OOM is
  contained to one client — there is no shared instance whose failure could affect
  others.
- **`close` is a disconnect hook, not a teardown you manage.** The per-connection
  process *is* the connection. The SDK monitors the writer, so one signal covers both a
  clean close and a dropped socket: `close` runs, then the process exits and exit
  cascades ([links](./links-and-supervision.md)) clean up anything it owned. Use `close`
  for application-level cleanup (leave a presence set, log the disconnect) — not to free
  platform resources, which the runtime reclaims regardless.
- **Shared state lives elsewhere.** Cross-connection state (presence counts, a shared
  log) belongs in a [service component](./lifecycle-service.md) or `kv`, never in the
  per-connection process. Group broadcast (a chat room fanning a message to its members)
  needs neither: **process-group tags** (`register-tag`/`whereis-tag`) are the platform
  primitive — each connection tags itself, membership auto-releases on exit.
- **Establishment cost is a spawn.** Each new connection is a fresh sandboxed
  process — the connection-storm benchmark measures exactly this
  (sandboxed-process-per-connection establishments per second).

Prev: [SSE component](./lifecycle-sse.md) · Next: [Worker component](./lifecycle-worker.md)
