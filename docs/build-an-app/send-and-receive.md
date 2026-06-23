# Send & receive

Processes communicate exclusively by passing messages — there's no shared memory, no
callback registrations, no event bus. One process puts a message in another process's
mailbox; the other reads it when it's ready. This page explains how that works, what
guarantees it gives you, and the patterns you'll use in practice.

## The mailbox

Every process has a **mailbox**: a FIFO queue of incoming messages. When you call
`send(pid, msg)`, the message is appended to that process's mailbox. That's it — `send`
returns immediately. The sender doesn't wait, doesn't get an acknowledgement, doesn't
know if the recipient is busy.

```
Process A                            Process B's mailbox
─────────────────────────────────    ──────────────────────
send(pidB, "job-1")   ──────────►   [ "job-1" ]
send(pidB, "job-2")   ──────────►   [ "job-1", "job-2" ]
                                     ...
                                     receive() → "job-1"
                                     receive() → "job-2"
```

The recipient calls `receive()` to take the next message. If the mailbox is empty,
`receive()` **blocks** — but cheaply. It doesn't block a thread; it suspends the
process's fiber, freeing the underlying worker for other processes. Other processes keep
running at full speed while this one waits.

## send — fire and forget

`send` is fire-and-forget:

- **Never blocks.** It enqueues and returns.
- **Never fails visibly.** If the recipient is dead, the message is silently dropped.
  No error, no exception. You don't need to guard every send with a liveness check.
- **Not ordered across processes.** Messages from one sender arrive in order to one
  recipient. But if two senders both send to the same recipient, their messages can
  interleave in any order.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Send a plain string:
Process.send(workerPid, "start");

// Send structured data — JSON is the idiomatic wire format:
Process.send(workerPid, JSON.stringify({ task: "resize", url: imageUrl, width: 800 }));
```

```rust [Rust]
// Send raw bytes:
rusm_rs::send_bytes(worker_pid, b"start");

// Send JSON:
let msg = serde_json::json!({ "task": "resize", "url": image_url, "width": 800 });
rusm_rs::send_bytes(worker_pid, msg.to_string().as_bytes());
```

```go [Go]
import (
    "encoding/json"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

// Send raw bytes:
rusm.Send(workerPid, []byte("start"))

// Send JSON:
msg, _ := json.Marshal(map[string]any{"task": "resize", "url": imageUrl, "width": 800})
rusm.Send(workerPid, msg)
```

:::

## receive — read the next message

`receive()` takes the next message from the mailbox and returns it as raw bytes. If
nothing is there yet, it waits.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Receive raw bytes (Uint8Array):
const raw = await Process.receive();
const msg = JSON.parse(new TextDecoder().decode(raw));

// Or receive directly as a string:
const text = await Process.receiveText();
const msg2 = JSON.parse(text);
```

```rust [Rust]
// Receive raw bytes:
let raw: Vec<u8> = rusm_rs::receive_bytes();

// Decode JSON:
let msg: serde_json::Value = serde_json::from_slice(&raw).unwrap();
```

```go [Go]
import "encoding/json"

// Receive raw bytes:
raw := rusm.Receive()

// Decode JSON:
var msg map[string]any
json.Unmarshal(raw, &msg)
```

:::

## Request / reply — the core pattern

The most common pattern is **request/reply**: process A sends a job and its own pid as
a reply address; process B does the work and sends the result back.

::: code-group

```ts [TypeScript]
// Process A — the caller:
import { Process } from "rusm-ts";

const me = Process.self();
Process.send(workerPid, JSON.stringify({ replyTo: me, input: "hello world" }));
const result = await Process.receiveText();   // blocks until the worker replies
console.log("result:", result);

// Process B — the worker:
import { Process } from "rusm-ts";

export default async function () {
  const raw = await Process.receiveText();
  const { replyTo, input } = JSON.parse(raw);
  const output = input.toUpperCase();         // do the work
  Process.send(replyTo, output);              // reply directly to the caller
}
```

```rust [Rust]
// Process A — the caller:
let me = rusm_rs::me();
let req = serde_json::json!({ "replyTo": me.to_string(), "input": "hello world" });
rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());
let result = rusm_rs::receive_bytes();
let result_str = String::from_utf8(result).unwrap();

// Process B — the worker:
#[rusm_rs::main]
fn run() {
    let raw = rusm_rs::receive_bytes();
    let msg: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let reply_to: rusm_rs::Pid = msg["replyTo"].as_str().unwrap().parse().unwrap();
    let output = msg["input"].as_str().unwrap().to_uppercase();
    rusm_rs::send_bytes(reply_to, output.as_bytes());
}
```

```go [Go]
// Process A — the caller:
me := rusm.Self()
req, _ := json.Marshal(map[string]any{"replyTo": me, "input": "hello world"})
rusm.Send(workerPid, req)
result := string(rusm.Receive())

// Process B — the worker:
func run() {
    raw := rusm.Receive()
    var msg struct {
        ReplyTo string `json:"replyTo"`
        Input   string `json:"input"`
    }
    json.Unmarshal(raw, &msg)
    output := strings.ToUpper(msg.Input)
    rusm.Send(rusm.Pid(msg.ReplyTo), []byte(output))
}
```

:::

::: tip Use the typed client for services
Raw send/receive is the low-level substrate. For calling a named service, `connect<T>`
(TS) or `Client::connect` (Rust) gives you a typed proxy — the request/reply pattern
above, hidden behind a regular function call. See
[Call another component](/build-an-app/call-another-component).
:::

## Mailbox capacity

By default, mailboxes are unbounded. A sender that fires faster than the recipient reads
will cause the mailbox to grow without limit — a memory leak in disguise.

For production services, cap the mailbox in `rusm.toml`. When the cap is reached, new
**user messages** are dropped (system signals like kill still get through). The service
stays alive and responsive; it just sheds excess load rather than accumulating memory.

```toml
[components.event-processor]
capability   = "sandboxed"
mailbox_capacity = 1000    # drop user messages beyond 1000 queued
```

Set this on any service that could receive bursts from many callers.

---

Next: [register & whereis](/build-an-app/register-and-whereis) — how to find a process
by name instead of tracking its pid.
