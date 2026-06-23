# Links & supervision

In most runtimes, a crash in a background worker is silent: the main thread keeps
running, the job never completes, the user gets a timeout minutes later. You find out
from a log line, if you're lucky.

RUSM treats crashes as **structured events**. You opt into how a crash propagates — via
a link (crash propagates to the partner) or a monitor (crash notifies the watcher). And
supervisors restart failed processes automatically, with configurable limits. The result
is **self-healing systems**: a process crashes, its supervisor notices, and a fresh
replacement is running before any human has to intervene.

## Links — crash propagation

A link is a **bidirectional bond** between two processes. If either process exits
abnormally — a crash, an unhandled error, a `kill` — the other receives an exit signal.
By default, that signal kills the receiver too.

This sounds harsh, but it's intentional. Linked processes are tightly coupled: if a
coordinator's worker crashes halfway through a job, the coordinator's state is now
invalid. Crashing together and restarting clean is safer than limping forward with
broken state.

Create a link at spawn time with `spawn_link`, or after the fact with `link(pid)`:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// spawn_link — the new process is linked to this one from the start.
// If the pipeline crashes, this coordinator crashes too (and will be restarted
// by its own supervisor with clean state).
const pipelinePid = Process.spawnLink("data-pipeline");

Process.send(pipelinePid, JSON.stringify({ dataset: "events-2025-06", output: "s3://bucket/out" }));
const summary = await Process.receiveText();
console.log("pipeline done:", summary);
```

```rust [Rust]
// spawn_link — linked from birth.
let pipeline_pid = rusm_rs::spawn_link("data-pipeline").expect("spawn failed");

rusm_rs::send_bytes(pipeline_pid, br#"{"dataset":"events-2025-06","output":"s3://bucket/out"}"#);
let summary = rusm_rs::receive_bytes();
println!("pipeline done: {}", String::from_utf8_lossy(&summary));
```

```go [Go]
import (
    "encoding/json"
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

// spawn_link — linked from birth.
pipelinePid, err := rusm.SpawnLink("data-pipeline")
if err != nil {
    panic(err)
}

req, _ := json.Marshal(map[string]any{"dataset": "events-2025-06", "output": "s3://bucket/out"})
rusm.Send(pipelinePid, req)
summary := string(rusm.Receive())
fmt.Println("pipeline done:", summary)
```

:::

## trap_exit — turn signals into messages

A process can opt out of dying on an exit signal by calling `trap_exit(true)`. Instead
of crashing, it receives the signal as a message in its mailbox — a `{ __exit: pid,
reason: "..." }` envelope — and can react: log the failure, restart the child, update
its state, or shut down gracefully.

This is exactly how a **supervisor** works: it links to all its children, traps exits,
and when a child dies it receives the signal and decides what to do next.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Trap exit signals — receive them as messages instead of dying.
Process.trapExit(true);

const workerPid = Process.spawnLink("file-processor");
Process.send(workerPid, JSON.stringify({ file: "/data/input.csv" }));

while (true) {
  const raw = await Process.receive();
  const msg = JSON.parse(new TextDecoder().decode(raw));

  if (msg.__exit) {
    // A linked process died. msg.__exit is the pid; msg.reason is the exit reason.
    console.error(`worker ${msg.__exit} exited: ${msg.reason}`);
    // Restart it, escalate, or exit cleanly — your choice.
    break;
  }

  // Normal message — handle it:
  console.log("result:", msg.output);
}
```

```rust [Rust]
// Trap exit signals.
rusm_rs::trap_exit(true);

let worker_pid = rusm_rs::spawn_link("file-processor").unwrap();
rusm_rs::send_bytes(worker_pid, br#"{"file":"/data/input.csv"}"#);

loop {
    let raw = rusm_rs::receive_bytes();

    // Check for an exit signal (__exit envelope):
    if let Some((dead_pid, reason)) = rusm_rs::exit_signal(&raw) {
        eprintln!("worker {} exited: {}", dead_pid, reason);
        break;
    }

    // Normal message:
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    println!("result: {}", result["output"]);
}
```

```go [Go]
import (
    "encoding/json"
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

// Trap exit signals.
rusm.TrapExit(true)

workerPid, _ := rusm.SpawnLink("file-processor")
rusm.Send(workerPid, []byte(`{"file":"/data/input.csv"}`))

for {
    raw := rusm.Receive()

    if deadPid, reason, ok := rusm.ExitSignal(raw); ok {
        fmt.Printf("worker %s exited: %s\n", deadPid, reason)
        break
    }

    var result map[string]any
    json.Unmarshal(raw, &result)
    fmt.Println("result:", result["output"])
}
```

:::

## The three supervision strategies

A supervisor is a process that links to a set of children, traps exits, and applies a
**restart strategy** when one dies. RUSM provides three:

### one-for-one — restart only the crashed child

Each child is independent. When one crashes, only it is restarted. The others keep
running exactly as before.

**Analogy:** a fleet of independent delivery drivers. One has a flat tyre — you send a
replacement driver. The others keep delivering.

**Use when:** children don't share state and a crash in one has no effect on the others.
HTTP request handlers, independent background jobs, per-user workers.

### one-for-all — restart every child

When any child crashes, **all** children are stopped and restarted together.

**Analogy:** a flight crew. If the co-pilot is incapacitated mid-flight, you don't keep
flying with just a pilot — you land and reassemble the full crew.

**Use when:** children share in-memory state, and a partial restart would leave the
group inconsistent. A pipeline where stage 2 holds state derived from stage 1's output:
if stage 1 crashes and restarts with clean state, stage 2's state is now stale and
wrong — restart them together.

### rest-for-one — restart the crashed child and its dependents

When a child crashes, it and all children started **after** it are restarted. Children
started before it are untouched.

**Analogy:** a production line. Station 3 breaks down — restart station 3 and all
downstream stations (4, 5, …), since they depend on station 3's output. Stations 1 and
2 (upstream) keep running.

**Use when:** children have an ordered dependency chain — later children depend on
earlier ones, but not vice versa.

---

Declare a supervision strategy in `rusm.toml`:

```toml
[components.coordinator]
capability = "sandboxed"
resident   = true          # boot-spawned and supervised by the node

[components.worker]
capability  = "sandboxed"
max_restarts       = 5     # at most 5 restarts …
restart_window_secs = 30   # … within a 30-second window
```

Or use the **in-guest `Supervisor`** to supervise children from inside a component —
no manifest entry needed for the children:

::: code-group

```ts [TypeScript]
import { Supervisor, Process } from "rusm-ts";

// A coordinator that supervises 3 workers under one-for-one.
export default async function () {
  const sup = new Supervisor("one-for-one");

  // Add children — each is a component name that will be spawned and supervised.
  sup.add("image-resizer");
  sup.add("video-transcoder");
  sup.add("pdf-generator");

  // Start the supervisor — spawns all children and begins watching them.
  await sup.start();

  // The supervisor restarts any child that crashes, up to the restart intensity limit.
  // This process stays alive as the supervisor loop runs.
}
```

```rust [Rust]
use rusm_rs::Supervisor;

#[rusm_rs::main]
fn run() {
    let mut sup = Supervisor::one_for_one();
    sup.add("image-resizer");
    sup.add("video-transcoder");
    sup.add("pdf-generator");

    // Blocks — runs the supervisor loop, restarting children as needed.
    sup.start();
}
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

func run() {
    sup := rusm.NewSupervisor(rusm.OneForOne)
    sup.Add("image-resizer")
    sup.Add("video-transcoder")
    sup.Add("pdf-generator")

    // Blocks — runs the supervisor loop.
    sup.Start()
}
```

:::

## Restart intensity — the circuit breaker

A naive supervisor would restart a broken child forever. If the child has a bug that
causes it to crash immediately on every start, you'd burn resources in a tight
restart loop.

**Restart intensity** is the limit: if a child crashes more than `max_restarts` times
within `restart_window_secs`, the supervisor gives up and exits itself — propagating
the failure upward to its own supervisor. This is cascading fault containment: a
repeated failure escalates until it reaches a level that can handle it (or the whole
node restarts cleanly).

```toml
[components.payment-processor]
capability          = "sandboxed"
max_restarts        = 3    # if it crashes 4 times …
restart_window_secs = 10   # … within 10 seconds, the supervisor gives up
```

Without this limit, a crash loop would silently burn CPU forever. With it, repeated
failures surface quickly and decisively.

## The supervision tree

Real systems compose supervisors into a **tree**. Each level handles the failures it can
recover from; failures it can't recover from propagate upward.

```
Node supervisor
├── API supervisor
│   ├── HTTP listener (resident)
│   └── Auth service (resident)
├── Worker supervisor
│   ├── image-resizer pool
│   ├── video-transcoder pool
│   └── pdf-generator pool
└── Storage supervisor
    ├── cache service (resident)
    └── KV flush worker
```

A single image-resizer crash is caught by the Worker supervisor and restarted silently.
If the entire image-resizer pool crashes in a loop (restart intensity exceeded), the
Worker supervisor exits and its parent is notified. The API supervisor and Storage
supervisor are completely unaffected. The failure is contained to the smallest scope
that can handle it.

This is the Erlang/OTP supervision philosophy — unchanged, applied to WebAssembly.

---

Next: [kill & killTag](/build-an-app/kill-and-killtag) — immediate termination and
group cancellation.
