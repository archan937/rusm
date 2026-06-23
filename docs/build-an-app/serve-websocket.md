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

Unlike SSE, WebSocket is **bidirectional**: the client sends frames and the server
receives them. But a WS handler is also a full actor process — so **`message` fires for
two distinct sources**:

- **A frame from the client** — the browser sent something.
- **An actor message from another process** — a peer connection relayed something via
  `Process.send` / `rusm_rs::send_bytes` (the same tag-broadcast mechanism SSE uses).

Both arrive as raw bytes. Your handler distinguishes them — typically by the shape of a
JSON envelope. This is what makes a chat room possible with no broker: each connection
joins a tag, clients broadcast via `whereisTag` + `send`, and peers' relays land in the
same mailbox.

Three callbacks — `open`, `message`, `close`:

- **`open(socket)`** — the client connected. `socket.send()` pushes a frame to this
  client. A good place to join a broadcast group with `registerTag`.
- **`message(socket, data)`** — either a client frame or an actor message arrived. Parse
  `data` to decide: if it's a command from the client, act on it (join a room, fan out to
  peers); if it's a relay from a peer, forward it to the client with `socket.send()`.
- **`close(socket)`** — the client disconnected. The tag membership releases
  automatically; this is optional cleanup.

**One handler instance per connection** — its state is private to that client.

Here's a minimal chat room — client sends `{"join":"<room>"}` then `{"say":"<text>"}`;
peers' relays arrive as `{"from":"<pid>","text":"..."}`:

::: code-group

```ts [TypeScript]
// components/chat/index.ts
import { websocket, Process, type Socket } from "rusm-ts";

let room: string | null = null; // this connection's current room (per-connection state)

const tag = (name: string) => `room:${name}`;
const system = (s: Socket, text: string) => s.send(JSON.stringify({ system: text }));

export default websocket({
  open(socket) {
    system(socket, 'connected — send {"join":"<room>"} to enter a room');
  },

  message(socket, data) {
    const msg = JSON.parse(new TextDecoder().decode(data));

    if (typeof msg.join === "string") {
      // Client wants to join a room: tag this process so broadcasts reach it.
      room = msg.join;
      Process.registerTag(tag(room));
      system(socket, `welcome to #${room}`);
      return;
    }

    if (typeof msg.say === "string") {
      // Client sent a chat message: fan it out to every connection in this room.
      if (!room) return system(socket, "join a room first");
      const relay = JSON.stringify({ from: String(Process.self()), text: msg.say });
      for (const pid of Process.whereisTag(tag(room))) Process.send(pid, relay);
      return;
    }

    // A relay from a peer arrived in the mailbox — forward it to this client.
    if (typeof msg.text === "string") socket.send(data);
  },

  close() {},
});
```

```rust [Rust]
// components/chat/src/lib.rs
use rusm_rs::ws::{self, Connection, Handler};
use serde::Deserialize;
use serde_json::json;

#[derive(Default)]
struct Chat { room: Option<String> }

#[derive(Deserialize)]
struct Frame { join: Option<String>, say: Option<String>, text: Option<String> }

impl Chat {
    fn tag(room: &str) -> String { format!("room:{room}") }
    fn system(conn: &Connection, text: &str) {
        conn.send(json!({ "system": text }).to_string().as_bytes());
    }
}

impl Handler for Chat {
    fn open(&mut self, conn: &Connection) {
        Self::system(conn, "connected — send {\"join\":\"<room>\"} to enter a room");
    }
    fn message(&mut self, conn: &Connection, data: Vec<u8>) {
        let Ok(frame) = serde_json::from_slice::<Frame>(&data) else { return };

        if let Some(room) = frame.join {
            // Client wants to join a room: tag this process so broadcasts reach it.
            rusm_rs::register_tag(&Self::tag(&room));
            Self::system(conn, &format!("welcome to #{room}"));
            self.room = Some(room);
            return;
        }
        if let Some(say) = frame.say {
            // Client sent a chat message: fan it out to every connection in this room.
            let Some(room) = &self.room else { return Self::system(conn, "join a room first") };
            let relay = json!({ "from": rusm_rs::me().to_string(), "text": say }).to_string();
            for pid in rusm_rs::whereis_tag(&Self::tag(room)) {
                rusm_rs::send_bytes(pid, relay.as_bytes());
            }
            return;
        }
        // A relay from a peer arrived in the mailbox — forward it to this client.
        if frame.text.is_some() { conn.send(&data); }
    }
    fn close(&mut self, _conn: &Connection) {}
}

#[rusm_rs::main]
fn run() { ws::serve(Chat::default()); }
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
