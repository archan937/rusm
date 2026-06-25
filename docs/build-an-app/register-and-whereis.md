# register & whereis

Every time a component is spawned, it gets a fresh pid — a new address that's never
been used before and won't be reused. That's great for workers you spawn and forget,
but it's a problem for **resident services**: if your counter service restarts (e.g.
after a crash), its pid changes. Any caller that saved the old pid is now pointing at
nothing.

The solution is **names**. A process claims a name once at startup; callers look it up
by name whenever they need to reach it. The name stays stable across restarts; the pid
behind it can change.

## register — claiming a name

A process calls `register(name)` to claim a name. From that moment, any other process
can find it by that name. A name can only be held by one process at a time. If the
process exits, the name is released automatically — no cleanup needed.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// In the service entry point — claim the name before doing any work:
Process.register("inventory");

// Now run the service loop — callers can find us while we're alive:
while (true) {
  const msg = JSON.parse(await Process.receiveText());
  // handle msg...
}
```

```rust [Rust]
#[rusm_rs::main]
fn run() {
    rusm_rs::register("inventory");

    loop {
        let raw = rusm_rs::receive_bytes();
        let msg: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        // handle msg...
    }
}
```

```go [Go]
func run() {
    rusm.Register("inventory")

    for {
        raw := rusm.Receive()
        var msg map[string]any
        json.Unmarshal(raw, &msg)
        // handle msg...
    }
}
```

:::

## whereis — looking up a name

Any process can resolve a name to a pid with `whereis`. If nothing is registered under
that name, it returns `null` / `None` / `false`. If the process has exited since the
last call, the name is already gone and `whereis` returns nothing.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const pid = Process.whereis("inventory");
if (pid) {
  Process.send(pid, JSON.stringify({ action: "reserve", sku: "ABC-1", qty: 3 }));
} else {
  console.error("inventory service not running");
}
```

```rust [Rust]
match rusm_rs::whereis("inventory") {
    Some(pid) => {
        let msg = serde_json::json!({ "action": "reserve", "sku": "ABC-1", "qty": 3 });
        rusm_rs::send_bytes(pid, msg.to_string().as_bytes());
    }
    None => {
        log::error!("inventory service not running");
    }
}
```

```go [Go]
pid, ok := rusm.Whereis("inventory")
if ok {
    msg, _ := json.Marshal(map[string]any{"action": "reserve", "sku": "ABC-1", "qty": 3})
    rusm.Send(pid, msg)
} else {
    slog.Error("inventory service not running")
}
```

:::

## One name, one pid at a time

Only one process can hold a name at a time. Attempting to register a name already held
by another process will fail. This is intentional — names are meant to be stable
singleton identities, not shared handles.

When a process exits (normally, by crash, or by `kill`), the name is released
immediately. The next process to `register` with that name takes it over — this is how
a resident service survives supervisor restarts cleanly.

## connect\<T\> — the ergonomic shortcut

Raw `whereis` + `send` is the low-level path. For services that follow the typed
service protocol (export functions → get a typed client), `connect<T>(name)` does the
`whereis` for you and returns a proxy whose method calls are real cross-process messages:

::: code-group

```ts [TypeScript]
import { connect } from "rusm-ts";
import type { Inventory } from "../inventory";

// connect() resolves the name and gives back a typed client:
const inv = connect<Inventory>("inventory");
const available = await inv.check("ABC-1");    // typed call, reply awaited
```

```rust [Rust]
// Client::connect resolves the name and returns a typed client:
let inv = inventory::Client::connect(rusm_rs::whereis("inventory").unwrap());
let available = inv.check("ABC-1").unwrap();
```

```go [Go]
pid, _ := rusm.Whereis("inventory")
available, _ := rusm.Call[bool](pid, "check", "ABC-1")
```

:::

The same client also does fire-and-forget casts, streaming results, and callbacks — the
full typed-client surface is in [Call another component](/build-an-app/call-another-component).

::: warning connect resolves the name — it does not verify liveness
`connect(name)` calls `whereis` once. If the service exits after that, subsequent calls
on the client will hang waiting for a reply that never comes. For resident services
supervised by the node this is rarely a problem — they restart quickly — but it's worth
knowing. See [Build a stateful service](/build-an-app/build-a-stateful-service).
:::

---

Next: [Tags & groups](/build-an-app/tags-and-groups) — when you need one name to reach
*many* processes at once.
