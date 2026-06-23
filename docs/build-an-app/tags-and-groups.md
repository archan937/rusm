# Tags & groups

The named registry maps **one name → one pid**. Sometimes you need the inverse: **one
name → many processes**. That's what tags are for.

A tag is a label a process attaches to itself. Any number of processes can share the
same tag. Anyone can ask which processes currently hold a tag, and send each of them a
message. When a process exits, its tags are released automatically — groups only ever
contain live processes.

This is RUSM's pub/sub. There's no broker, no topic object, no subscription list to
manage. The group *is* the set of processes that tagged themselves. Broadcasting *is*
a loop over `whereisTag` + `send`.

## Joining and leaving a group

A process joins a group by tagging itself. It can join multiple groups. Leaving is
explicit with `unregisterTag`, or automatic when the process exits.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Join a group — typically in your open() callback or at startup:
Process.registerTag("notifications:user-123");
Process.registerTag("broadcast:all-users");   // can hold multiple tags

// Leave explicitly if needed before process exit:
Process.unregisterTag("notifications:user-123");
```

```rust [Rust]
// Join a group:
rusm_rs::register_tag("notifications:user-123");
rusm_rs::register_tag("broadcast:all-users");

// Leave explicitly:
rusm_rs::unregister_tag("notifications:user-123");
```

```go [Go]
// Join a group:
rusm.RegisterTag("notifications:user-123")
rusm.RegisterTag("broadcast:all-users")

// Leave explicitly:
rusm.UnregisterTag("notifications:user-123")
```

:::

## Broadcasting to a group

`whereisTag(tag)` returns the current list of live pids in the group. Broadcast by
looping over them and sending each one a message.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const payload = JSON.stringify({ type: "price-update", sku: "ABC-1", price: 29.99 });

for (const pid of Process.whereisTag("broadcast:all-users")) {
  Process.send(pid, payload);
}
```

```rust [Rust]
let payload = serde_json::json!({ "type": "price-update", "sku": "ABC-1", "price": 29.99 });
let bytes = payload.to_string();

for pid in rusm_rs::whereis_tag("broadcast:all-users") {
    rusm_rs::send_bytes(pid, bytes.as_bytes());
}
```

```go [Go]
payload, _ := json.Marshal(map[string]any{"type": "price-update", "sku": "ABC-1", "price": 29.99})

for _, pid := range rusm.WhereisTag("broadcast:all-users") {
    rusm.Send(pid, payload)
}
```

:::

The loop is a snapshot — it captures who's in the group at that moment. A process that
exits mid-loop is skipped silently (sending to a dead pid is always a no-op).

## Use case 1: a chat room

Each WebSocket connection is its own process. When a connection joins a room, it tags
itself. When any connection says something, it broadcasts to the room's tag:

::: code-group

```ts [TypeScript]
import { websocket, Process } from "rusm-ts";

let room: string | null = null;

export default websocket({
  open(socket) { /* greet */ },

  message(socket, data) {
    const msg = JSON.parse(new TextDecoder().decode(data));

    if (typeof msg.join === "string") {
      room = msg.join;
      Process.registerTag(`room:${room}`);   // join — this connection is now a group member
      return;
    }

    if (typeof msg.say === "string" && room) {
      // Broadcast to every connection in this room:
      const relay = JSON.stringify({ from: String(Process.self()), text: msg.say });
      for (const pid of Process.whereisTag(`room:${room}`)) {
        Process.send(pid, relay);
      }
    }
  },

  close() {},   // tag released automatically on disconnect
});
```

```rust [Rust]
// In the WebSocket handler's message callback:
if let Some(room_name) = frame.join {
    rusm_rs::register_tag(&format!("room:{}", room_name));
    self.room = Some(room_name);
}

if let Some(text) = frame.say {
    if let Some(room) = &self.room {
        let relay = serde_json::json!({ "from": rusm_rs::me().to_string(), "text": text });
        let bytes = relay.to_string();
        for pid in rusm_rs::whereis_tag(&format!("room:{}", room)) {
            rusm_rs::send_bytes(pid, bytes.as_bytes());
        }
    }
}
```

```go [Go]
// In the WebSocket handler's Message callback:
if msg.Join != "" {
    rusm.RegisterTag("room:" + msg.Join)
    room = msg.Join
}

if msg.Say != "" && room != "" {
    relay, _ := json.Marshal(map[string]any{
        "from": rusm.Self(),
        "text": msg.Say,
    })
    for _, pid := range rusm.WhereisTag("room:" + room) {
        rusm.Send(pid, relay)
    }
}
```

:::

No broker. No pub/sub middleware. Each connection is a process; the group is the room.

## Use case 2: scoped cancellation

Tag every process that belongs to one unit of work — all the agents working on a plan,
all the background jobs for one user request. To cancel the whole unit, call `killTag`.
One call, instant, authoritative.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Each agent tags itself with the plan it's working on:
Process.registerTag(`plan:${planId}`);

// The coordinator cancels everything for that plan in one call:
Process.killTag(`plan:${planId}`);
```

```rust [Rust]
// Each agent:
rusm_rs::register_tag(&format!("plan:{}", plan_id));

// The coordinator cancels the whole plan:
rusm_rs::kill_tag(&format!("plan:{}", plan_id));
```

```go [Go]
// Each agent:
rusm.RegisterTag("plan:" + planID)

// The coordinator:
rusm.KillTag("plan:" + planID)
```

:::

No cancel tokens, no polling, no bookkeeping of which pids to stop. The platform owns
the group registry and the kill. The application writes two calls.

## Zero overhead on untagged processes

Tags are stored in a per-process list. A process that holds no tags pays nothing — no
allocation, no map lookup on the hot path. The group registry only touches a process
at join, leave, and exit. Running thousands of untagged processes costs exactly as
much as without tags.

---

Next: [kill & killTag](/build-an-app/kill-and-killtag) — stopping processes immediately
or in groups, and when to reach for supervision instead.
