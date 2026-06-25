# Links & supervision

In most runtimes, a crash in a background worker is silent: the main thread keeps
running, the job never completes, and the user gets a timeout minutes later. You find out
from a log line, if you're lucky.

RUSM treats a crash as a **structured event**. A failure doesn't vanish — it becomes a
signal something can act on. From a guest you reach for one of two tools: a **monitor**,
when you want to *notice* a death and react to it yourself, or a **supervisor**, when you
want a dead process *restarted automatically*. Together they give you self-healing
systems: a process crashes, its supervisor notices, and a fresh replacement is running
before any human has to intervene.

This page is about the second tool — supervision. (For the first, see
[Monitors](/build-an-app/monitors).)

## The model underneath: links, exits, trapping

A supervisor isn't magic; it's built on the runtime's crash-propagation model. Worth
understanding even though you rarely touch it directly:

- A **link** is a bidirectional bond between two processes. If either exits abnormally,
  the other receives an exit signal — and, by default, exits too. Tightly-coupled
  processes crash together and restart clean, rather than one limping on with state that
  depends on the other.
- A process can **trap exits**: instead of dying on a signal, it receives the exit as a
  message and decides what to do — log it, restart the child, escalate, shut down.

That pair — link plus trap-exit — *is* a supervisor: it links to its children, traps
their exits, and restarts them by a strategy. RUSM's core implements this so you don't
have to wire it by hand; from a guest you use the in-guest **`Supervisor`** below. For
the mechanism itself, see [links & supervision](/deep-dive/links-and-supervision) in the
deep dive.

## The three supervision strategies

A supervisor watches a set of children and, when one dies, applies a **restart
strategy**. RUSM provides the three classic OTP strategies:

### one-for-one — restart only the crashed child

Each child is independent. When one crashes, only it is restarted; the others keep
running exactly as before.

**Analogy:** a fleet of independent delivery drivers. One gets a flat tyre — you send a
replacement. The others keep delivering.

**Use when:** children don't share state and a crash in one has no effect on the others —
HTTP request handlers, independent background jobs, per-user workers.

### one-for-all — restart every child

When any child crashes, **all** children are stopped and restarted together.

**Analogy:** a flight crew. If the co-pilot is incapacitated mid-flight, you don't keep
flying with just a pilot — you land and reassemble the full crew.

**Use when:** children share in-memory state and a partial restart would leave the group
inconsistent. A pipeline where stage 2 holds state derived from stage 1: if stage 1
restarts clean, stage 2's state is now stale — restart them together.

### rest-for-one — restart the crashed child and its dependents

When a child crashes, it and every child started **after** it are restarted. Children
started before it are untouched.

**Analogy:** a production line. Station 3 breaks down — restart station 3 and everything
downstream (4, 5, …), since they depend on its output. Stations 1 and 2 keep running.

**Use when:** children form an ordered dependency chain — later ones depend on earlier
ones, but not the reverse.

## The in-guest Supervisor

Supervise children from inside a component with the in-guest `Supervisor` — the children
are component names, spawned and watched for you, restarted by the strategy. Run it as the
component body:

::: code-group

```ts [TypeScript]
import { supervise } from "rusm-ts";

export default async function () {
  await supervise({
    strategy: "one_for_one",          // "one_for_one" | "one_for_all" | "rest_for_one"
    children: ["image-resizer", "video-transcoder", "pdf-generator"],
    maxRestarts: 5,                   // restart-intensity ceiling (see below)
    maxSeconds: 30,                   // … within a 30-second sliding window
  });
}
```

```rust [Rust]
use std::time::Duration;
use rusm_rs::supervisor::{Strategy, Supervisor};

#[rusm_rs::main]
fn run() {
    Supervisor::new(Strategy::OneForOne)
        .child("image-resizer")
        .child("video-transcoder")
        .child("pdf-generator")
        .max_restarts(5)                  // restart-intensity ceiling …
        .within(Duration::from_secs(30))  // … within a 30-second sliding window
        .run();                           // blocks: spawns the children and supervises them
}
```

```go [Go]
import (
    "time"

    rusm "github.com/archan937/rusm/packages/rusm-go"
)

func run() {
    rusm.Supervisor{
        Strategy:    rusm.OneForOne, // rusm.OneForOne | OneForAll | RestForOne
        Children:    []string{"image-resizer", "video-transcoder", "pdf-generator"},
        MaxRestarts: 5,                // restart-intensity ceiling …
        Within:      30 * time.Second, // … within a 30-second sliding window
    }.Run() // blocks: spawns the children and supervises them
}
```

:::

Mark the supervising component **resident** so the node boot-spawns it — and supervises
*it* in turn, so even the supervisor self-heals:

```toml
[components.coordinator]
capability = "sandboxed"
resident   = true   # boot-spawned at startup and supervised by the node
```

The children need no manifest entry of their own — the supervisor spawns them by name.
(A child *does* need to be a registered component; `rusm build` registers everything under
`components/`.)

## Restart intensity — the circuit breaker

A naive supervisor would restart a broken child forever. If a child has a bug that crashes
it immediately on every start, that's a tight loop burning CPU.

**Restart intensity** is the limit. Configure it on the supervisor — `maxRestarts` within
`maxSeconds` (TS), `max_restarts` within `within` (Rust), `MaxRestarts` within `Within`
(Go). If a child exceeds the ceiling inside the window, the supervisor stops trying and
exits itself — propagating the failure to *its* supervisor. That's cascading fault
containment: a repeated failure escalates until it reaches a level that can handle it (or
the node restarts cleanly). Leave the ceiling unset for unlimited restarts; set it for
anything that could crash-loop.

## The supervision tree

Real systems compose supervisors into a **tree**. Each level handles the failures it can
recover from; the rest propagate upward.

```
Node supervisor
├── API supervisor
│   ├── HTTP listener (resident)
│   └── Auth service (resident)
├── Worker supervisor
│   ├── image-resizer
│   ├── video-transcoder
│   └── pdf-generator
└── Storage supervisor
    ├── cache service (resident)
    └── KV flush worker
```

A single image-resizer crash is caught by the Worker supervisor and restarted silently.
If the whole image-resizer line crash-loops (restart intensity exceeded), the Worker
supervisor exits and its parent is notified — while the API and Storage supervisors carry
on untouched. The failure is contained to the smallest scope that can handle it.

This is the Erlang/OTP supervision philosophy, unchanged — applied to WebAssembly.

---

Next: [kill & killTag](/build-an-app/kill-and-killtag) — immediate termination and group
cancellation.
