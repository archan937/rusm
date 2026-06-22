# Receive a broadcast

The flip side of [Broadcast to many](/build-an-app/broadcast-to-many): once you've tagged
yourself into a group, a broadcast is **just a message in your mailbox**. There is no special
"subscriber" object to read — you handle it with the same `receive` (or the same `message`
callback) you'd use for any message. How you do it depends on what kind of process you are.

## A connection handler — handle it in `message`

If the subscriber is a per-connection [SSE](/build-an-app/serve-sse) or
[WebSocket](/build-an-app/serve-websocket) handler, you don't write a loop at all: subscribe in
`open`, and the host delivers each broadcast to your `message` callback (push, not polling).
That's the full pattern shown on those two pages — `open` calls `register_tag`, `message` fires
once per broadcast and emits it to the client.

## A plain process — read it from the mailbox

A process that isn't a connection handler subscribes once, then loops on `receive`. Each
broadcast arrives like any other message, in order, and `receive` **parks** the fiber until one
lands — no busy-wait:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

Process.registerTag("room:lobby");                  // subscribe once
for (;;) {
  const msg = JSON.parse(await Process.receiveText());
  handle(msg);                                      // a broadcast (or anything sent to me)
}
```

```rust [Rust]
rusm_rs::register_tag("room:lobby");                 // subscribe once
loop {
    let msg = rusm_rs::receive_bytes();             // parks until a message arrives
    handle(&msg);
}
```

```go [Go]
rusm.RegisterTag("room:lobby")                       // subscribe once
for {
    msg := rusm.ReceiveBytes()
    handle(msg)
}
```

:::

## Tell messages apart

A process has **one mailbox**, so broadcasts from every group it joined — plus any direct
sends and call replies — arrive together. Put a `type` (or `kind`) on the payload and match on
it; the SDKs decode the JSON wire in one step:

::: code-group

```ts [TypeScript]
const m = JSON.parse(await Process.receiveText());
switch (m.type) {
  case "chat": render(m.text); break;
  case "join": addMember(m.who); break;
}
```

```rust [Rust]
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Chat { text: String },
    Join { who: String },
}

match rusm_rs::receive::<Event>() {       // decode JSON straight into the enum
    Ok(Event::Chat { text }) => render(&text),
    Ok(Event::Join { who }) => add_member(&who),
    Err(_) => {} // not one of ours — ignore
}
```

```go [Go]
type Event struct {
    Type string `json:"type"`
    Text string `json:"text"`
    Who  string `json:"who"`
}

if ev, err := rusm.Receive[Event](); err == nil {   // decode JSON into the struct
    switch ev.Type {
    case "chat":
        render(ev.Text)
    case "join":
        addMember(ev.Who)
    }
}
```

:::

## What you need to know

- **No subscriber API — it's the mailbox.** `register_tag` only adds you to the group;
  receiving a broadcast uses the *same* `receive` / `message` as any message. Publishing is a
  plain `send` to each member ([Broadcast to many](/build-an-app/broadcast-to-many)).
- **Push, ordered, no spin.** Messages arrive in send order; `receive` (and the `message`
  callback) park the fiber until one lands — never a polling loop.
- **One mailbox, many groups.** A process in several tags — or one that also gets direct
  messages or call replies — reads them all from the same mailbox, so tag the payload to
  dispatch.
- **Leaving the group.** A process leaves every group automatically when it exits (nothing to
  prune). To leave one while staying alive, `unregister_tag("room:lobby")`.
- **Also making calls or supervising children?** Don't hand-roll a bare `receive` loop — let
  the SDK multiplex call replies and `__down` signals for you. See
  [Build a stateful service](/build-an-app/build-a-stateful-service) and
  [Coordinate & supervise](/build-an-app/coordinate-and-supervise).
