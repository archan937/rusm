# Processes & messaging

Every component you write runs inside a **process** — RUSM's fundamental unit of
execution. Processes are lightweight (thousands fit comfortably in memory), isolated
(they share nothing), and communicate exclusively by passing messages. This page
explains the vocabulary you'll see everywhere: what a Pid is, how mailboxes work,
what tags are, and which `Process` calls do what.

## What is a process?

Think of a process as a **tiny, isolated program** running on its own. It has:

- its own **memory** — no other process can read or write it
- its own **mailbox** — a queue of incoming messages
- a **pid** — a unique identity you can address it by

When a process finishes (or crashes), it's gone. Nothing leaks into other processes.
That isolation is what makes RUSM safe to scale: a bug in one process cannot corrupt
another.

## Pid — process identity

A **Pid** (process identifier) is the address of one running process. It's an opaque
value — under the hood a small integer assigned by the runtime when a process is
spawned. You never construct one yourself; you get one back from `spawn`, `self()`,
`whereis`, or `whereisTag`.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const me = Process.self();   // the pid of THIS process, e.g. "42"
```

```rust [Rust]
let me: rusm_rs::Pid = rusm_rs::me();   // the Pid of this process
```

```go [Go]
me := rusm.Self()   // the Pid of this process
```

:::

A pid is only meaningful while the process lives. Once it exits, the pid is gone —
sending to a dead pid is a no-op, not an error.

## Mailboxes — how messages queue up

Every process has a **mailbox**: a FIFO queue of incoming messages. When you `send` a
message to another process, it lands in that process's mailbox. The recipient picks
it up by calling `receive`, which blocks until a message is available.

```
Process A                           Process B
─────────────────────────────────   ─────────────────────────────────
send(pidB, "hello")   ──────────►  mailbox: ["hello"]
                                    ...
                                    receive()  ──► "hello"
```

`send` is **fire-and-forget**: it never blocks, never waits for the recipient, and
never fails visibly if the recipient is slow or dead. The message is simply queued (or
silently dropped if the recipient is gone).

Mailboxes are unbounded by default. For production services you can cap the depth in
`rusm.toml` (`mailbox_capacity = 1000`) so a flooded service sheds load instead of
accumulating memory.

## Sending and receiving

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Send a message (JSON string) to another process:
Process.send(otherPid, JSON.stringify({ action: "ping" }));

// Receive the next message — blocks until one arrives:
const raw = await Process.receive();                        // Uint8Array
const msg = JSON.parse(new TextDecoder().decode(raw));      // { action: "ping" }

// Or as text directly:
const text = await Process.receiveText();                   // string
```

```rust [Rust]
use rusm_rs::{send_bytes, receive_bytes, me};

// Send raw bytes to another process:
send_bytes(other_pid, b"hello");

// Receive the next message — blocks until one arrives:
let data: Vec<u8> = receive_bytes();
let text = String::from_utf8(data).unwrap();   // "hello"
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

// Send a message to another process:
rusm.Send(otherPid, []byte("hello"))

// Receive the next message — blocks until one arrives:
data := rusm.Receive()                          // []byte
text := string(data)                            // "hello"
```

:::

## self() — who am I?

`self()` returns the pid of the *currently running* process. The most common use is
telling another process where to send its reply:

::: code-group

```ts [TypeScript]
const me = Process.self();
Process.send(workerPid, JSON.stringify({ replyTo: me, job: "compress" }));

// Later, the worker sends back to `me` and we receive it:
const result = await Process.receiveText();
```

```rust [Rust]
let me = rusm_rs::me();
rusm_rs::send_bytes(worker_pid, format!(r#"{{"replyTo":"{}","job":"compress"}}"#, me).as_bytes());

let result = rusm_rs::receive_bytes();
```

```go [Go]
me := rusm.Self()
rusm.Send(workerPid, []byte(fmt.Sprintf(`{"replyTo":"%s","job":"compress"}`, me)))

result := rusm.Receive()
```

:::

## register & whereis — named processes

Pids change every time a process is spawned. If you always need to reach the *same*
resident service, hardcoding a pid is fragile. Instead, a process **claims a name**
once at startup:

::: code-group

```ts [TypeScript]
// In the service — claim a name so callers can find you:
Process.register("counter");

// In any caller — look it up by name:
const pid = Process.whereis("counter");   // Pid | null
if (pid) Process.send(pid, "bump");
```

```rust [Rust]
// In the service:
rusm_rs::register("counter");

// In any caller:
if let Some(pid) = rusm_rs::whereis("counter") {
    rusm_rs::send_bytes(pid, b"bump");
}
```

```go [Go]
// In the service:
rusm.Register("counter")

// In any caller:
if pid, ok := rusm.Whereis("counter"); ok {
    rusm.Send(pid, []byte("bump"))
}
```

:::

One name → one pid at a time. When the process exits, the name is released
automatically. `connect<T>("counter")` is the ergonomic shortcut for callers — it
calls `whereis` for you and gives back a typed client.

## Tags — one name, many processes

The registry maps one name to one pid. **Tags** map one name to *many* pids. A process
tags *itself*; any number of processes can share the same tag; and you can broadcast to
all of them at once.

::: code-group

```ts [TypeScript]
// Join a group (in each connection's open callback, for example):
Process.registerTag("chat:general");

// Find every process in the group:
const members = Process.whereisTag("chat:general");   // Pid[]

// Broadcast a message to all of them:
const payload = JSON.stringify({ from: Process.self(), text: "hello" });
for (const pid of Process.whereisTag("chat:general")) {
  Process.send(pid, payload);
}
```

```rust [Rust]
// Join a group:
rusm_rs::register_tag("chat:general");

// Find every process in the group:
let members: Vec<rusm_rs::Pid> = rusm_rs::whereis_tag("chat:general");

// Broadcast:
let payload = format!(r#"{{"from":"{}","text":"hello"}}"#, rusm_rs::me());
for pid in rusm_rs::whereis_tag("chat:general") {
    rusm_rs::send_bytes(pid, payload.as_bytes());
}
```

```go [Go]
// Join a group:
rusm.RegisterTag("chat:general")

// Find every process in the group:
members := rusm.WhereisTag("chat:general")   // []Pid

// Broadcast:
payload := fmt.Sprintf(`{"from":"%s","text":"hello"}`, rusm.Self())
for _, pid := range rusm.WhereisTag("chat:general") {
    rusm.Send(pid, []byte(payload))
}
```

:::

When a process exits, its tag memberships are released automatically — you never need
to leave a group explicitly. A process can hold any number of tags; tags add zero
overhead to processes that hold none.

## kill & killTag — stopping processes

`kill(pid)` stops one process immediately. `killTag(tag)` stops **every process in a
group** at once — the clean primitive for scoped cancellation: tag every process that
belongs to one unit of work, and cancel the whole unit with one call.

::: code-group

```ts [TypeScript]
// Stop one process:
Process.kill(workerPid);

// Stop every process tagged "plan:abc123" (e.g. all agents for one request):
Process.killTag("plan:abc123");
```

```rust [Rust]
// Stop one process:
rusm_rs::kill(worker_pid);

// Stop every process in the group:
rusm_rs::kill_tag("plan:abc123");
```

```go [Go]
// Stop one process:
rusm.Kill(workerPid)

// Stop every process in the group:
rusm.KillTag("plan:abc123")
```

:::

Both require the `process-control` capability — a sandboxed guest cannot kill other
processes by default.

## isAlive — liveness check

::: code-group

```ts [TypeScript]
if (Process.isAlive(pid)) {
  Process.send(pid, "ping");
}
```

```rust [Rust]
if rusm_rs::is_alive(pid) {
    rusm_rs::send_bytes(pid, b"ping");
}
```

```go [Go]
if rusm.IsAlive(pid) {
    rusm.Send(pid, []byte("ping"))
}
```

:::

A process can exit between `isAlive` and `send`. Sending to a dead pid is always a
no-op, so for most cases you can skip the check and just send.

## setLabel — name a process for the observer

Labels appear in the live observer and dashboard — useful when you have many processes
and want to tell them apart at a glance. They don't affect routing or messaging.

::: code-group

```ts [TypeScript]
Process.setLabel(`agent:pages/${subjectId}`);
```

```rust [Rust]
rusm_rs::set_label(&format!("agent:pages/{subject_id}"));
```

```go [Go]
rusm.SetLabel(fmt.Sprintf("agent:pages/%s", subjectID))
```

:::

## Quick reference

| What you want | TypeScript | Rust | Go |
|---|---|---|---|
| My pid | `Process.self()` | `rusm_rs::me()` | `rusm.Self()` |
| Send a message | `Process.send(pid, msg)` | `rusm_rs::send_bytes(pid, bytes)` | `rusm.Send(pid, bytes)` |
| Receive next message | `await Process.receive()` | `rusm_rs::receive_bytes()` | `rusm.Receive()` |
| Receive as text | `await Process.receiveText()` | `String::from_utf8(receive_bytes())` | `string(rusm.Receive())` |
| Claim a name | `Process.register(name)` | `rusm_rs::register(name)` | `rusm.Register(name)` |
| Look up by name | `Process.whereis(name)` | `rusm_rs::whereis(name)` | `rusm.Whereis(name)` |
| Join a group | `Process.registerTag(tag)` | `rusm_rs::register_tag(tag)` | `rusm.RegisterTag(tag)` |
| List a group | `Process.whereisTag(tag)` | `rusm_rs::whereis_tag(tag)` | `rusm.WhereisTag(tag)` |
| Leave a group | `Process.unregisterTag(tag)` | `rusm_rs::unregister_tag(tag)` | `rusm.UnregisterTag(tag)` |
| Kill one process | `Process.kill(pid)` | `rusm_rs::kill(pid)` | `rusm.Kill(pid)` |
| Kill a whole group | `Process.killTag(tag)` | `rusm_rs::kill_tag(tag)` | `rusm.KillTag(tag)` |
| Liveness check | `Process.isAlive(pid)` | `rusm_rs::is_alive(pid)` | `rusm.IsAlive(pid)` |
| Set observer label | `Process.setLabel(label)` | `rusm_rs::set_label(label)` | `rusm.SetLabel(label)` |
| List all pids | `Process.list()` | `rusm_rs::list()` | `rusm.List()` |

For supervision, monitors, and links — reacting when a process dies — see
[Coordinate & supervise](/build-an-app/coordinate-and-supervise). For byte streams
(large payloads without copying into the mailbox), see
[Byte streams](/deep-dive/byte-streams).
