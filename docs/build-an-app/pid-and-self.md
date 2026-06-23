# Pid & self

Every running process has a **Pid** (process identifier) — its unique address on the
node. You'll see pids everywhere: as return values from `spawn`, as the argument to
`send`, as the identity you register under a name. This page explains exactly what a
Pid is, how to get yours, and what happens when you use a pid that's no longer alive.

## What a Pid looks like

A Pid is an opaque string — in practice a small decimal integer like `"1"`, `"42"`, or
`"1007"`. The runtime assigns them sequentially as processes are spawned. You never
construct one by hand; you always receive one from the runtime.

The value itself doesn't tell you anything about what the process is doing, what
component it runs, or where it lives. It's just an address. The runtime keeps the
mapping from pid to running task.

## Getting your own pid — `self()`

Every process can ask the runtime: *"What is my address?"* That's `self()`. The most
common use is to give other processes a return address — a pid they can reply to.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const me = Process.self();
console.log(`I am process ${me}`);   // e.g. "I am process 42"

// Give a worker our pid so it knows where to send the result:
Process.send(workerPid, JSON.stringify({ replyTo: me, job: "transcode" }));

// Block until the worker replies to us:
const result = await Process.receiveText();
```

```rust [Rust]
let me = rusm_rs::me();
log::info!("I am process {}", me);

// Give a worker our pid as the reply-to address:
rusm_rs::send_bytes(
    worker_pid,
    format!(r#"{{"replyTo":"{}","job":"transcode"}}"#, me).as_bytes(),
);

// Block until the worker replies:
let result = rusm_rs::receive_bytes();
```

```go [Go]
import (
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

me := rusm.Self()
fmt.Printf("I am process %s\n", me)

// Give a worker our pid as the reply-to address:
rusm.Send(workerPid, []byte(fmt.Sprintf(`{"replyTo":"%s","job":"transcode"}`, me)))

// Block until the worker replies:
result := rusm.Receive()
```

:::

## Dead pids — why sending is always safe

A pid is only meaningful while its process lives. Once a process exits — normally,
by crash, or by `kill` — the pid is gone. There's no registry entry, no mailbox.

If you send to a dead pid, **nothing bad happens**. The message is silently dropped.
No error, no panic, no exception. This is by design: the sender often doesn't know
whether the recipient is still alive, and requiring a liveness check before every
`send` would be both awkward and inherently racy (the process could die between the
check and the send anyway).

If you *need* to know whether a process is alive before doing something meaningful with
the result, use `isAlive` — but most of the time you don't need it:

::: code-group

```ts [TypeScript]
// Most of the time — just send, it's safe:
Process.send(somePid, "ping");

// When you genuinely need to act differently if it's gone:
if (Process.isAlive(somePid)) {
  Process.send(somePid, JSON.stringify({ action: "update", value: 42 }));
} else {
  // re-spawn, alert, fall back — your call
}
```

```rust [Rust]
// Just send — it's always safe:
rusm_rs::send_bytes(some_pid, b"ping");

// When you need to branch on liveness:
if rusm_rs::is_alive(some_pid) {
    rusm_rs::send_bytes(some_pid, b"{\"action\":\"update\",\"value\":42}");
} else {
    // re-spawn or handle absence
}
```

```go [Go]
// Just send — it's always safe:
rusm.Send(somePid, []byte("ping"))

// When you need to branch on liveness:
if rusm.IsAlive(somePid) {
    rusm.Send(somePid, []byte(`{"action":"update","value":42}`))
} else {
    // re-spawn or handle absence
}
```

:::

::: warning isAlive is not a lock
There's an inherent race: `isAlive` returns `true`, then the process exits before your
`send` arrives. That's fine — the send is still a no-op. Use `isAlive` for
human-readable diagnostics or branching logic, not as a safety gate.
:::

## setLabel — naming a process for the observer

Pids like `"42"` are not helpful when you're staring at a live dashboard with hundreds
of running processes. `setLabel` attaches a human-readable name to your process — it
shows up in the observer, the dashboard, and log lines stamped with the process identity.

Labels don't affect routing or messaging. They're purely for observability.

::: code-group

```ts [TypeScript]
// Set this early in your entry point, before doing real work:
Process.setLabel(`agent:pages/${subjectId}`);
```

```rust [Rust]
rusm_rs::set_label(&format!("agent:pages/{subject_id}"));
```

```go [Go]
rusm.SetLabel(fmt.Sprintf("agent:pages/%s", subjectID))
```

:::

---

Next: [Send & receive](/build-an-app/send-and-receive) — how messages actually flow
between processes.
