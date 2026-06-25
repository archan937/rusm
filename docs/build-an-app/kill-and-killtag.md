# kill & killTag

Sometimes a process needs to stop another process — to cancel work, clean up a
connection, or tear down a whole unit of concurrent activity. RUSM gives you two
primitives for this: `kill` for one process, `killTag` for an entire group.

## kill — stop one process immediately

`kill(pid)` aborts a process right now. It doesn't drain its mailbox, doesn't wait for
it to finish its current work, doesn't give it a chance to clean up. It's an immediate,
unconditional stop.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Spawn a long-running job:
const jobPid = Process.spawn("video-encoder");
Process.send(jobPid, JSON.stringify({ file: "input.mp4", preset: "hq" }));

// User cancelled — stop it now:
Process.kill(jobPid);
```

```rust [Rust]
let job_pid = rusm_rs::spawn("video-encoder").unwrap();
rusm_rs::send_bytes(job_pid, br#"{"file":"input.mp4","preset":"hq"}"#);

// Cancel immediately:
rusm_rs::kill(job_pid);
```

```go [Go]
jobPid, _ := rusm.Spawn("video-encoder")
rusm.Send(jobPid, []byte(`{"file":"input.mp4","preset":"hq"}`))

// Cancel immediately:
rusm.Kill(jobPid)
```

:::

After `kill` returns, the process is gone. Sending to its pid is a no-op.

## Stopping a process gracefully

`kill` is immediate and unconditional — it doesn't let the process finish its current
message or clean up. When a clean exit matters (flushing writes, closing a connection,
emitting a final log line), don't kill it: send a message it recognises as "stop" and let
it leave its own receive loop. Cooperative shutdown is just an ordinary message the process
chooses to act on.

::: code-group

```ts [TypeScript]
// The worker treats a "stop" message as its cue to exit cleanly:
const msg = JSON.parse(await Process.receiveText());
if (msg.op === "stop") return;     // leaves the loop → the process ends here
// ...otherwise do the work

// To stop it, send that message instead of killing it:
Process.send(workerPid, JSON.stringify({ op: "stop" }));
```

```rust [Rust]
let msg: serde_json::Value = serde_json::from_slice(&rusm_rs::receive_bytes()).unwrap();
if msg["op"] == "stop" { return; }  // clean exit

rusm_rs::send_bytes(worker_pid, br#"{"op":"stop"}"#);
```

```go [Go]
var msg struct{ Op string `json:"op"` }
json.Unmarshal(rusm.Receive(), &msg)
if msg.Op == "stop" { return }      // clean exit

rusm.Send(workerPid, []byte(`{"op":"stop"}`))
```

:::

**When to use which:**

| Situation | Use |
|---|---|
| User cancelled a request mid-flight | `kill` — stop immediately |
| External connection dropped | `kill` — nothing to flush |
| Worker needs to flush a buffer before stopping | a cooperative `stop` message |
| Resident service being replaced on redeploy | a cooperative `stop` message |
| Something crashed and you need its resources released | `kill` |

## killTag — stop an entire group at once

`killTag(tag)` kills every process currently in the named group. All of them,
simultaneously, in one call. This is RUSM's **scoped cancellation primitive**.

The pattern: tag every process that belongs to one logical unit of work. When that unit
needs to be cancelled, call `killTag` on the shared tag.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Each agent spawned for a plan tags itself at startup:
Process.registerTag(`plan:${planId}`);

// The HTTP cancel endpoint stops the entire plan:
export function cancel(planId: string): void {
  const stopped = Process.killTag(`plan:${planId}`);
  console.log(`cancelled ${stopped} agents for plan ${planId}`);
}
```

```rust [Rust]
// Each agent:
rusm_rs::register_tag(&format!("plan:{}", plan_id));

// The cancel handler:
let stopped = rusm_rs::kill_tag(&format!("plan:{}", plan_id));
log::info!("cancelled {} agents for plan {}", stopped, plan_id);
```

```go [Go]
// Each agent:
rusm.RegisterTag("plan:" + planID)

// The cancel handler:
stopped := rusm.KillTag("plan:" + planID)
slog.Info("cancelled agents", "count", stopped, "plan", planID)
```

:::

`killTag` returns how many processes it killed. That's also your signal that the
cancellation is complete — all those processes are gone by the time the call returns.

## Capability gate — sandboxed guests can't kill by default

Both `kill` and `killTag` require the **`process-control`** capability. A sandboxed
guest (the default profile) cannot terminate other processes. This is intentional: a
sandboxed component running untrusted or third-party code should not be able to take
down other components.

To grant `kill` / `killTag` to a component, either use a profile that includes
`process-control` or define a custom profile:

```toml
[capabilities.orchestrator]
inherits = "sandboxed"
allow-process-control = true   # may kill / killTag / list other processes

[components.plan-coordinator]
capability = "orchestrator"    # this component can kill
```

A component that lacks the capability gets `0` back from `killTag` and a no-op from
`kill` — it fails silently rather than throwing.

## kill vs supervision

`kill` and `killTag` are for **intentional cancellation** — you made a decision that
work should stop. They're not for handling failures.

For handling the case where a process *unexpectedly* crashes or exits, use
**supervision**: monitor a process, and when it goes down, restart it or react. That's
the subject of [Coordinate & supervise](/build-an-app/coordinate-and-supervise).

The rule of thumb:

- **You decided to stop it** → `kill` / `killTag`
- **It stopped unexpectedly** → supervision / monitors

---

Next: [Coordinate & supervise](/build-an-app/coordinate-and-supervise) — reacting when
processes exit unexpectedly, and building self-healing trees of processes.
