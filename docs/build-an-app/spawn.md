# Spawn

`spawn` is how you create a new process. Call it with the name of a registered
component; it starts the component in a fresh, isolated process and returns the new
process's pid immediately. Your caller keeps running — the spawned process runs
concurrently, independently, in its own memory.

## Basic spawn

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// Start a new "image-resizer" process — returns as soon as the process is created.
const resizerPid = Process.spawn("image-resizer");

// The resizer is now running concurrently. Send it a job:
Process.send(resizerPid, JSON.stringify({
  replyTo: String(Process.self()),
  url: "https://example.com/photo.jpg",
  width: 800,
}));

// Wait for the result:
const result = await Process.receiveText();
console.log("resized:", result);
```

```rust [Rust]
let resizer_pid = rusm_rs::spawn("image-resizer").expect("image-resizer not registered");

// Send it a job:
let job = serde_json::json!({
    "replyTo": rusm_rs::me().to_string(),
    "url": "https://example.com/photo.jpg",
    "width": 800,
});
rusm_rs::send_bytes(resizer_pid, job.to_string().as_bytes());

// Wait for the result:
let result = rusm_rs::receive_bytes();
println!("resized: {}", String::from_utf8_lossy(&result));
```

```go [Go]
import (
    "encoding/json"
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

resizerPid, err := rusm.Spawn("image-resizer")
if err != nil {
    panic(fmt.Sprintf("image-resizer not registered: %v", err))
}

// Send it a job:
job, _ := json.Marshal(map[string]any{
    "replyTo": rusm.Self(),
    "url":     "https://example.com/photo.jpg",
    "width":   800,
})
rusm.Send(resizerPid, job)

// Wait for the result:
result := string(rusm.Receive())
fmt.Println("resized:", result)
```

:::

The spawned component (`image-resizer`) runs its own `default` export / `run` function.
It reads from its mailbox, does work, and sends replies. It knows nothing about who
spawned it — the only connection is the messages you exchange.

## Know when a spawned child exits — monitor

`spawn` gives you a child that runs independently; it doesn't tell you when that child
stops. To find out, **monitor** it. After `monitor(pid)`, the moment that process exits —
cleanly, by crash, or by `kill` — a `__down` message lands in your mailbox carrying its pid
and exit reason. No polling, no watcher process.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const worker = Process.spawn("data-pipeline");
Process.monitor(worker);                       // a `__down` message arrives when it exits

Process.send(worker, JSON.stringify({ dataset: "sales-q4", format: "parquet" }));
const msg = JSON.parse(await Process.receiveText());
if (msg.__down) {
  // the worker exited before replying — restart, log, or give up
}
```

```rust [Rust]
let worker = rusm_rs::spawn("data-pipeline").expect("spawn failed");
rusm_rs::monitor(worker);                       // a `__down` message arrives when it exits

rusm_rs::send_bytes(worker, br#"{"dataset":"sales-q4","format":"parquet"}"#);
let output = rusm_rs::receive_bytes();
```

```go [Go]
worker, _ := rusm.Spawn("data-pipeline")
rusm.Monitor(worker)                            // a `__down` message arrives when it exits

rusm.Send(worker, []byte(`{"dataset":"sales-q4","format":"parquet"}`))
output := rusm.Receive()
```

:::

To turn a monitor into automatic restarts — a real supervision tree — use the in-guest
`Supervisor`. See [Coordinate & supervise](/build-an-app/coordinate-and-supervise).

## spawn_from — dynamic components

`spawn_from` runs a **runtime-chosen** compiled Wasm component or JS bundle under a
pre-declared template profile. The caller picks the code; the operator fixes what that
code is allowed to do.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

// `spawn` takes an optional source — that second argument turns it into spawn-from.
// Run a compiled plugin from the node's durable KV store:
const pluginPid = Process.spawn("plugin-runner", "kv:plugins/greeter");

// Or a bundle fetched from a URL (re-fetched after TTL, cached by content hash):
const remotePid = Process.spawn("plugin-runner", "url:https://cdn.example/plugin.wasm");

Process.send(pluginPid, JSON.stringify({ name: "Alice" }));
const greeting = await Process.receiveText();
```

```rust [Rust]
// From the KV store:
let plugin_pid = rusm_rs::spawn_from("plugin-runner", "kv:plugins/greeter")
    .expect("spawn_from failed");

// From a URL:
let remote_pid = rusm_rs::spawn_from("plugin-runner", "url:https://cdn.example/plugin.wasm")
    .expect("spawn_from failed");

rusm_rs::send_bytes(plugin_pid, br#"{"name":"Alice"}"#);
let greeting = rusm_rs::receive_bytes();
```

```go [Go]
// From the KV store:
pluginPid, err := rusm.SpawnFrom("plugin-runner", "kv:plugins/greeter")
if err != nil {
    panic(err)
}

rusm.Send(pluginPid, []byte(`{"name":"Alice"}`))
greeting := string(rusm.Receive())
```

:::

The `plugin-runner` template is declared in `rusm.toml` with `dynamic = "wasm"` (or
`"js"`). The capability profile on the template is what the spawned code runs under —
the caller cannot widen it:

```toml
[components.plugin-runner]
capability = "sandboxed"   # no network, no storage — locked in by the operator
dynamic    = "wasm"
```

See [Dynamic WASM](/build-an-app/dynamic-wasm) for the full story.

## Capabilities gate spawn

A component must have the `spawn` capability to call `spawn` at all. Without it, the
call is refused at the ABI boundary before any guest code runs.

```toml
[capabilities.orchestrator]
inherits = "sandboxed"
allow-spawn = true         # this component may spawn others

[components.coordinator]
capability = "orchestrator"

[components.image-resizer]
capability = "sandboxed"   # the resizer's OWN profile — not inherited from the spawner
```

The spawned component (`image-resizer`) always runs under its **own** manifest-declared
profile, regardless of who spawned it. A coordinator with `trusted` capabilities cannot
grant those capabilities to a child by spawning it — the child's profile is fixed at
registration time.

## spawn vs connect

| | `spawn` | `connect` |
|---|---|---|
| **Creates** | a fresh process | attaches to an existing one |
| **Use for** | workers, per-request handlers, one-off jobs | resident services, long-lived singletons |
| **Pid** | new every call | same pid for the life of the service |

One-line rule: **spawn** when you want new work done; **connect** when you want to talk
to work already running. See [Call another component](/build-an-app/call-another-component).

---

Next: [register & whereis](/build-an-app/register-and-whereis) — stable names for
processes that need to be found without holding a pid.
