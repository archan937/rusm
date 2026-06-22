# Broadcast to many

To push one message to many processes at once — a chat room to its members, a live feed to
every open [SSE](/build-an-app/serve-sse) or [WebSocket](/build-an-app/serve-websocket)
connection — use **process-group tags**: RUSM's built-in pub/sub. There is no broker and no
shared list. A process **subscribes** by tagging itself; a publisher **broadcasts** to the
tag, and the message lands in every tagged process's mailbox. Membership auto-releases when a
process exits, so nothing leaks.

## Subscribe — tag yourself

A subscriber (often a per-connection SSE/WS handler) registers the tag in `open`:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";
Process.registerTag("room:lobby");        // I'm now a member of "room:lobby"
```

```rust [Rust]
rusm_rs::register_tag("room:lobby");
```

```go [Go]
rusm.RegisterTag("room:lobby")
```

:::

## Publish — broadcast to the tag

Any process (a service, an HTTP handler) looks up the tag's members and sends each one — that
*is* the broadcast:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";
for (const pid of Process.whereisTag("room:lobby")) {
  Process.send(pid, payload);             // each subscriber's mailbox / SSE message fires
}
```

```rust [Rust]
for pid in rusm_rs::whereis_tag("room:lobby") {
    rusm_rs::send(pid, &payload).ok();
}
```

```go [Go]
for _, pid := range rusm.WhereisTag("room:lobby") {
	rusm.Send(pid, payload)
}
```

:::

That's the whole pattern: a per-connection [SSE](/build-an-app/serve-sse) handler subscribes in
`open` and emits in `message`; a [stateful service](/build-an-app/build-a-stateful-service) or an HTTP
handler publishes. Each open stream's `message` fires — push, not polling, exactly as live as
the publisher.

## What you need to know

- **No broker, no leak.** Tags are membership over real pids; when a subscriber exits (a
  client disconnects), it leaves the group automatically. You never prune a stale list.
- **Targeted kill, too.** The same group is addressable for control: `kill_tag("room:lobby")`
  ends every member at once — handy for "cancel everything for this job" (gated by the
  `process-control` capability).
- **It's the actor model.** `register-tag` / `whereis-tag` / `kill-tag` are part of the
  [process API](/build-an-app/coordinate-and-supervise); pub/sub is just send over a group.
