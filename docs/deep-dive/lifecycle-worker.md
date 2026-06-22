# Lifecycle — Worker component (non-serving, per-call)

A short-lived process a sibling `spawn`s to do **one unit of work** and exit. No socket
and no listener — pure actor work, off the serving path. See the
[overview](/deep-dive/component-lifecycle) for the shared two-domain model and failure
vocabulary.

## Shape (what you write)

*A complete worker wired into a real app is the `reporter` component of the todo-board examples — [TypeScript](https://github.com/archan937/rusm/tree/main/examples/typescript/components/reporter) · [Rust](https://github.com/archan937/rusm/tree/main/examples/rust/components/reporter) · [Go](https://github.com/archan937/rusm/tree/main/examples/go/components/reporter).*

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// The process body: receive one job, do it, reply, then return (the process exits).
export default async function () {
  const job = JSON.parse(await Process.receiveText()); // blocks; the fiber parks
  const result = doWork(job);
  Process.send(job.replyTo, JSON.stringify(result));   // reply to the caller
}
```

```rust [Rust]
#[rusm_rs::main]
fn run() {
    let job: Job = rusm_rs::receive().unwrap(); // the work item (blocks; the fiber parks)
    let result = do_work(&job);                 // your logic
    rusm_rs::send(job.reply_to, &result).ok();  // reply to the caller
    // returning ends the process — it exits Normal
}
```

```go [Go]
package main

import rusm "github.com/archan937/rusm/packages/rusm-go"

func init() { rusm.Run(run) }
func main() {}

func run() {
	job, _ := rusm.Receive[Job]() // the work item (blocks; the fiber parks)
	result := doWork(job)         // your logic
	rusm.Send(job.ReplyTo, result) // reply to the caller
	// returning ends the process — it exits Normal
}
```

:::

A worker is spawned on demand by another process (`rusm_rs::spawn("worker")` in Rust,
`spawn("worker")` in TypeScript, `rusm.Spawn("worker")` in Go), does its job —
optionally streaming results back over
a [byte stream](/deep-dive/byte-streams) or messages — and returns. The dispatch-from-
`commander` pattern in the [app model](/deep-dive/app-model) is exactly this.

## Platform owns / you write

- **Platform owns:** the **capability-gated** spawn — the `allow-spawn` capability gates who
  may spawn, and a node-registered worker runs under **its own manifest-declared
  profile** (an ad-hoc registration inherits the spawner's caps) — delivering the work
  message, scheduling/parking the fiber on blocking calls, and notifying monitors/links
  on exit.
- **You write:** receive the job, do it, (optionally) reply, return.

## Lifecycle events

| Event | Platform domain | Application domain | Result |
| --- | --- | --- | --- |
| **Normal** | spawn → deliver the job → reclaim on return | does the work, returns | result delivered; the process is gone |
| **Spawn denied** | a parent without the `allow-spawn` capability gets an error (not a new process) | the `spawn(...)` call returns `Err` | no worker — the parent decides what to do |
| **Crash (trap)** | the process is Crashed; a spawner that `monitor`ed it gets a `Down(reason)`; a **linked** spawner gets an exit signal (or an exit cascade) | the `panic!` / `.unwrap()` | surfaced to the spawner: retry / give up / escalate |
| **Memory crash (OOM)** | the `StoreLimiter` cap trips a trap → Crashed → `Down` | exceeded `max-memory-mb` | same — surfaced to the spawner |
| **Kill** (parent died, via a link) | the exit cascade fires this process's abort handle | — | reclaimed; no orphan |

## Notes

- **Supervision is opt-in and yours to shape.** A worker isn't auto-restarted; the
  *spawner* chooses the policy by `monitor`ing it (a dead child arrives as a `__down`
  message) or by putting it under an in-guest
  [`Supervisor`](/deep-dive/links-and-supervision) with a one-for-one / rest-for-one
  strategy.
- **Secrets stay scoped.** A node-registered worker runs under its **declared**
  profile, so a worker that needs (say) an API key gets it from its own
  `[capabilities.<name>]` — the spawner needn't hold that key. A guest still can't
  fabricate capabilities the operator never declared, and the `allow-spawn` capability gates
  who may spawn at all.
- **Concurrency = many workers.** A guest is single-threaded; you get parallelism by
  spawning *more* workers (each its own process/instance), not by threading inside one.

Prev: [WebSocket component](/deep-dive/lifecycle-websocket) · Next: [Service component](/deep-dive/lifecycle-service)
