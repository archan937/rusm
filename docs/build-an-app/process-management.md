# Process management

A component imports the `rusm:runtime/actor` interface and calls the Erlang
`Process` API directly — the same operations the host has:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const me = Process.self;                    // self()
Process.register("worker");                 // name yourself in the registry
const who = Process.whereis("worker");      // look a name up → bigint | null
const all = Process.list();                 // every live pid (find all)
const info = Process.info(me);              // links, label, mailbox depth… | null
const alive = Process.isAlive(somePid);
Process.send(somePid, bytes);               // message-pass (bytes or text)
const incoming = await Process.receive();   // await the next message
Process.kill(somePid);                      // terminate another process
Process.unregister("worker");
Process.setLabel("worker#1");               // a human label for the observer
```

```rust [Rust]
use rusm::runtime::actor;

let me = actor::own_pid();                 // self()
actor::register("worker");                 // name yourself in the registry
let who = actor::whereis("worker");        // look a name up → Option<pid>
let all = actor::list_processes();         // every live pid (find all)
let info = actor::info(me);                // Option<process-info>: links, label, mailbox depth…
let alive = actor::is_alive(some_pid);
actor::send(some_pid, &bytes);             // message-pass (bytes)
let incoming = actor::receive();           // block for the next message
actor::kill(some_pid);                     // terminate another process
actor::unregister("worker");
actor::set_label("worker#1");              // a human label for the observer
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

me := rusm.Self()                  // self()
rusm.Register("worker")            // name yourself in the registry
who, ok := rusm.Whereis("worker")  // look a name up → (Pid, bool)
all := rusm.List()                 // every live pid (find all)
info, ok := rusm.Info(me)          // (ProcessInfo, bool): links, label, mailbox depth…
alive := rusm.IsAlive(somePid)
rusm.SendBytes(somePid, bytes)     // message-pass (bytes)
incoming := rusm.ReceiveBytes()    // block for the next message
rusm.Kill(somePid)                 // terminate another process
rusm.Unregister("worker")
rusm.SetLabel("worker#1")          // a human label for the observer
```

:::

The runnable proof is the `actor-echo` test fixture, which drives **every** op
from inside a real component.

> **Spawn-from-guest is supported — capability-gated.** A component declared in
> `rusm.toml` can be `spawn`ed **by name** from inside another component (`spawn`
> in the actor ABI), so you get per-request workers and concealed typed clients —
> the Erlang model. It's default-deny (the `allow-spawn` capability gates *who* may spawn);
> a **node-registered** component runs under **its own manifest-declared profile**
> (what the manifest declares is what runs, whoever spawns it), so secrets stay scoped
> to the component that needs them — never the spawner. Components still find
> long-lived peers with `register`/`whereis` and talk with `send`/`receive`; a
> request/reply "callback" is just a message and a reply. See
> [components & the actor world](/deep-dive/components-and-the-actor-world).

## Streaming between processes

Beyond messages, two processes can share a **byte stream** — a back-pressured pipe for a
large or open-ended payload (a file, an event feed, an LLM token stream) you don't want to
buffer as one message. It's a low-level primitive: most apps stream to *clients* through the
[serving](/build-an-app/serving) path (SSE/WS bodies ride a byte stream under the hood) and never touch it
directly. When you genuinely need a direct process-to-process pipe, the producer/consumer
API and its real use cases are documented in [byte streams](/deep-dive/byte-streams).
