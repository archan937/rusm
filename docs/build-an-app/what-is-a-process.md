# What is a process?

In RUSM, a **process** is the fundamental unit of execution. Every component you run —
an HTTP handler, a background worker, a resident service, a WebSocket connection — is a
process. Understanding what a process is and how it differs from other concurrency models
is the key to understanding everything else in RUSM.

## The mental model

A process is a **tiny, isolated program** running on its own. It has three things:

- **Its own memory.** No other process can read or write it. There are no shared variables,
  no global state visible across processes. If you want to share data, you pass a message.
- **Its own mailbox.** An inbox where other processes deliver messages. The process reads
  them one at a time, in order, at its own pace.
- **Its own pid.** A unique identity so others can address it. Think of it as a phone
  number — you dial it to reach that specific process.

When a process crashes, **nothing leaks into other processes**. Its memory is gone, its
mailbox is gone, and any process watching it (via a monitor or link) is notified — but
every other process keeps running exactly as before. This is isolation by construction,
not by discipline.

## How it differs from threads

With threads you write concurrent code by sharing memory. Two threads can read and write
the same variable. That's powerful but treacherous: you need locks, and forgetting one
means a data race that corrupts state silently.

RUSM processes share **nothing**. There's no lock to forget. The only way for two
processes to interact is to send each other messages. This eliminates an entire category
of bugs — and it's why you can run thousands of processes without worrying about who
mutates what.

## How it differs from stateless request handlers

A traditional web handler is spawned per request, handles it, and disappears — no
memory across requests. RUSM has this too (HTTP handlers are per-request processes), but
processes can also be **long-lived and stateful**: a resident service process runs for
the life of the node, accumulating state in its own memory, answering messages from
many callers. No database needed for in-memory shared state.

## Spawning a process

You start a new process with `spawn`. It receives the name of a registered component and
returns the new process's pid. The spawned process runs its entry point concurrently —
your caller doesn't wait for it to finish.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Spawn a component by its registered name — returns immediately with the new pid.
const workerPid = Process.spawn("image-resizer");

// The worker is now running concurrently. Send it a job:
Process.send(workerPid, JSON.stringify({ url: "https://example.com/photo.jpg", width: 800 }));
```

```rust [Rust]
// Spawn a component — returns the new process's Pid.
let worker_pid = rusm_rs::spawn("image-resizer").expect("spawn failed");

// Send it work:
rusm_rs::send_bytes(worker_pid, br#"{"url":"https://example.com/photo.jpg","width":800}"#);
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

// Spawn a component — returns the new process's Pid.
workerPid, err := rusm.Spawn("image-resizer")
if err != nil { /* handle */ }

// Send it work:
rusm.Send(workerPid, []byte(`{"url":"https://example.com/photo.jpg","width":800}`))
```

:::

## Lifecycle

A process has a simple lifecycle:

1. **Spawned** — the runtime creates it and assigns it a pid.
2. **Running** — it executes its entry point, reads its mailbox, does work.
3. **Exits** — either normally (entry point returns) or abnormally (a crash / `kill`).

After exit, the pid is gone. Sending to it is a no-op. If another process was watching
it via a monitor, it receives a `__down` notification — so failures propagate only to
those who opted in.

## How many can you run?

A lot. Processes are backed by Tokio tasks — they're cheap to create and suspend. A
RUSM node has spawned over **440,000 component processes per second** on a laptop under
benchmark. In practice the limit is memory and file descriptors, not a fixed cap you
need to worry about for normal applications.

---

Next: [Pid & self](/build-an-app/pid-and-self) — the process identity in detail.
