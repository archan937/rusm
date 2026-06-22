# Coordinate & supervise

Underneath the typed clients and serving handlers is the Erlang **process toolkit** — find,
message, watch, and supervise processes. You reach for it directly when you build coordination
of your own: a registry, a watchdog, a supervision tree.

## The process API

A component imports `rusm:runtime/actor` and calls the same operations the host has:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const me = Process.self;                  // self()
Process.register("worker");               // name yourself in the registry
const who = Process.whereis("worker");    // look a name up → bigint | null
const all = Process.list();               // every live pid
Process.send(somePid, bytes);             // message-pass
const msg = await Process.receive();       // await the next message
Process.kill(somePid);                    // terminate another process
Process.setLabel("worker#1");             // a label for the observer
```

```rust [Rust]
use rusm::runtime::actor;

let me = actor::own_pid();
actor::register("worker");
let who = actor::whereis("worker");       // Option<pid>
let all = actor::list_processes();
actor::send(some_pid, &bytes);
let msg = actor::receive();               // blocks (the fiber parks)
actor::kill(some_pid);
actor::set_label("worker#1");
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

me := rusm.Self()
rusm.Register("worker")
who, ok := rusm.Whereis("worker")         // (Pid, bool)
all := rusm.List()
rusm.SendBytes(somePid, bytes)
msg := rusm.ReceiveBytes()                // blocks
rusm.Kill(somePid)
rusm.SetLabel("worker#1")
```

:::

`register`/`whereis` is how long-lived components find each other without a central registry;
`register-tag`/`whereis-tag` is the group form behind [Broadcast to many](/build-an-app/broadcast-to-many).

## Watch for failure — monitors & links

To react when another process dies, **monitor** it: its death arrives as a `__down` message
in your mailbox (no polling, no watcher process). A **link** is bidirectional — a crash
propagates to linked peers (unless they trap exits), which is how a failure tears down a whole
group cleanly.

::: code-group

```ts [TypeScript]
const child = Process.spawn("worker");
Process.monitor(child);            // a `__down` message arrives when it exits
```

```rust [Rust]
let child = rusm_rs::spawn("worker").unwrap();
rusm_rs::monitor(child);           // a `__down` message arrives when it exits
```

```go [Go]
child, _ := rusm.Spawn("worker")
rusm.Monitor(child)                // a `__down` message arrives when it exits
```

:::

When the child exits, the next `receive` yields a `__down` carrying the pid and exit reason —
handle it (restart, log, give up).

## Supervise — restart what breaks

Rather than wire monitors by hand, use the in-guest **`Supervisor`** (in `rusm-ts`,
`rusm-rs`, and `rusm-go`): it spawns named children, monitors them, and restarts per strategy
— `one_for_one` (restart just the dead child), `one_for_all` (restart them all), or
`rest_for_one` — with a `max_restarts` window so a crash-looping child gives up instead of
spinning. This is the OTP supervision tree, written from inside a guest:

::: code-group

```ts [TypeScript]
import { supervise } from "rusm-ts";

export default async function () {
  await supervise({
    strategy: "one_for_one",
    maxRestarts: 5,
    children: ["store", "reporter"],   // spawn + monitor + restart these by name
  });
}
```

```rust [Rust]
use rusm_rs::supervisor::{Supervisor, Strategy};

#[rusm_rs::main]
fn run() {
    Supervisor::new(Strategy::OneForOne)
        .child("store")
        .child("reporter")
        .max_restarts(5)
        .run();
}
```

```go [Go]
func run() {
	rusm.Supervisor{
		Strategy:    rusm.OneForOne,
		Children:    []string{"store", "reporter"},
		MaxRestarts: 5,
	}.Run()
}
```

:::

A `resident = true` component is already supervised by the node (see
[Build a stateful service](/build-an-app/build-a-stateful-service)); reach for your own `Supervisor`
when you need a custom tree or strategy inside a component.

## What you need to know

- **`kill` / `monitor` / `list` are capability-gated** — controlling *other* processes needs
  the `process-control` capability (see [Grant capabilities](/build-an-app/grant-capabilities)).
- **Failure is a message, not an exception across the wire.** A crash exits the process; links
  and monitors turn that into signals you handle — supervisors restart exactly what broke,
  the rest keeps running.
- **Going deeper:** the model behind all this is in
  [links & supervision](/deep-dive/links-and-supervision) and
  [message passing](/deep-dive/message-passing).
