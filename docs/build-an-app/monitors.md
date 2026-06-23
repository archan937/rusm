# Monitors

You spawned a worker, sent it a job, and you're waiting for the reply. What if the
worker crashes before it can respond? Without any safeguard, your `receive()` blocks
forever — a silent hang with no error.

A **monitor** solves this. Call `monitor(pid)` and the runtime watches that process for
you. The moment it exits — whether it finishes normally, crashes, or is killed — a
`__down` message lands in your mailbox. You can race your receive loop against it: if
the result arrives, great; if `__down` arrives first, the worker is gone and you handle
the failure.

## monitor vs links

Two ways to watch a process:

| | Monitor | Link |
|---|---|---|
| **Direction** | one-way — watcher stays alive | bidirectional — both crash |
| **On exit** | `__down` message arrives in your mailbox | exit signal kills you too (unless trapping) |
| **Use when** | you want to *react* to a death, stay running | two processes are so coupled neither makes sense alone |

Use **monitors** when you're a coordinator waiting on a worker. Use **links** (and
supervision) when a crash on either side should propagate. See
[Links & supervision](/build-an-app/links-and-supervision).

## Setting up a monitor

`monitor(pid)` returns a **monitor reference** — an opaque handle you can use to
cancel the monitor later. You can monitor any live pid, including processes you didn't
spawn yourself.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const workerPid = Process.spawn("thumbnail-generator");

// Watch the worker — if it exits for any reason, we'll get a "__down" message.
const monRef = Process.monitor(workerPid);

// Cancel the monitor when we no longer need it:
Process.demonitor(monRef);
```

```rust [Rust]
let worker_pid = rusm_rs::spawn("thumbnail-generator").unwrap();

// Watch the worker:
let mon_ref = rusm_rs::monitor(worker_pid);

// Cancel when done:
rusm_rs::demonitor(mon_ref);
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

workerPid, _ := rusm.Spawn("thumbnail-generator")

// Watch the worker:
monRef := rusm.Monitor(workerPid)

// Cancel when done:
rusm.Demonitor(monRef)
```

:::

## The safe request/reply pattern

Here's the full pattern: spawn a worker, monitor it, send a job, then in the receive
loop handle either the successful reply or the `__down` crash notification.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

async function generateThumbnail(imageUrl: string): Promise<string | null> {
  const workerPid = Process.spawn("thumbnail-generator");
  const monRef = Process.monitor(workerPid);

  // Send the job — include our pid so the worker knows where to reply.
  Process.send(workerPid, JSON.stringify({
    replyTo: String(Process.self()),
    url: imageUrl,
  }));

  // Wait for the reply OR a crash notification.
  const raw = await Process.receive();
  const msg = JSON.parse(new TextDecoder().decode(raw));

  if (msg.__down) {
    // The worker crashed before replying.
    console.error(`thumbnail-generator crashed: ${msg.__down}`);
    return null;
  }

  // Got the result — cancel the monitor so no stale __down arrives later.
  Process.demonitor(monRef);
  return msg.thumbnailUrl as string;
}
```

```rust [Rust]
fn generate_thumbnail(image_url: &str) -> Option<String> {
    let worker_pid = rusm_rs::spawn("thumbnail-generator").ok()?;
    let mon_ref = rusm_rs::monitor(worker_pid);

    // Send the job:
    let req = serde_json::json!({
        "replyTo": rusm_rs::me().to_string(),
        "url": image_url,
    });
    rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());

    // Receive: either the result or a __down message.
    let raw = rusm_rs::receive_bytes();

    // Check if it's a __down signal:
    if let Some(dead_pid) = rusm_rs::down_pid(&raw) {
        eprintln!("thumbnail-generator crashed (pid {})", dead_pid);
        return None;
    }

    // Got the result — cancel the monitor.
    rusm_rs::demonitor(mon_ref);

    let reply: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    Some(reply["thumbnailUrl"].as_str()?.to_string())
}
```

```go [Go]
import (
    "encoding/json"
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

func generateThumbnail(imageUrl string) (string, error) {
    workerPid, err := rusm.Spawn("thumbnail-generator")
    if err != nil {
        return "", err
    }
    monRef := rusm.Monitor(workerPid)

    // Send the job:
    req, _ := json.Marshal(map[string]any{
        "replyTo": rusm.Self(),
        "url":     imageUrl,
    })
    rusm.Send(workerPid, req)

    // Receive: either the result or a __down.
    raw := rusm.Receive()

    if downPid, ok := rusm.DownPid(raw); ok {
        return "", fmt.Errorf("thumbnail-generator crashed: pid %s", downPid)
    }

    // Got the result — cancel the monitor.
    rusm.Demonitor(monRef)

    var reply map[string]any
    json.Unmarshal(raw, &reply)
    return reply["thumbnailUrl"].(string), nil
}
```

:::

## Combine with a timeout timer

For a complete safety net, combine a monitor with a timer. You're now protected against
both a crash (monitor's `__down`) and a hang (timer's `"timeout"` message):

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

async function generateThumbnailSafe(imageUrl: string): Promise<string | null> {
  const workerPid = Process.spawn("thumbnail-generator");
  const monRef   = Process.monitor(workerPid);
  const timer    = Process.sendAfter(5_000, Process.self(), "timeout");

  Process.send(workerPid, JSON.stringify({ replyTo: String(Process.self()), url: imageUrl }));

  const raw = await Process.receive();
  const msg = JSON.parse(new TextDecoder().decode(raw));

  if (msg === "timeout" || msg.__down) {
    if (msg === "timeout") Process.kill(workerPid); // worker is still running but too slow
    Process.demonitor(monRef);
    return null;
  }

  Process.demonitor(monRef);
  Process.cancelTimer(timer);
  return msg.thumbnailUrl as string;
}
```

```rust [Rust]
fn generate_thumbnail_safe(image_url: &str) -> Option<String> {
    let worker_pid = rusm_rs::spawn("thumbnail-generator").ok()?;
    let mon_ref = rusm_rs::monitor(worker_pid);
    let timer   = rusm_rs::send_after(5_000, rusm_rs::me(), b"timeout");

    let req = serde_json::json!({ "replyTo": rusm_rs::me().to_string(), "url": image_url });
    rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());

    let raw = rusm_rs::receive_bytes();

    if raw == b"timeout" {
        rusm_rs::kill(worker_pid);
        rusm_rs::demonitor(mon_ref);
        return None;
    }
    if let Some(_) = rusm_rs::down_pid(&raw) {
        rusm_rs::cancel_timer(timer);
        return None;
    }

    rusm_rs::demonitor(mon_ref);
    rusm_rs::cancel_timer(timer);
    let reply: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    Some(reply["thumbnailUrl"].as_str()?.to_string())
}
```

```go [Go]
func generateThumbnailSafe(imageUrl string) (string, error) {
    workerPid, _ := rusm.Spawn("thumbnail-generator")
    monRef := rusm.Monitor(workerPid)
    timer  := rusm.SendAfter(5_000, rusm.Self(), []byte("timeout"))

    req, _ := json.Marshal(map[string]any{"replyTo": rusm.Self(), "url": imageUrl})
    rusm.Send(workerPid, req)

    raw := rusm.Receive()

    if string(raw) == "timeout" {
        rusm.Kill(workerPid)
        rusm.Demonitor(monRef)
        return "", fmt.Errorf("thumbnail-generator timed out")
    }
    if downPid, ok := rusm.DownPid(raw); ok {
        rusm.CancelTimer(timer)
        return "", fmt.Errorf("thumbnail-generator crashed: pid %s", downPid)
    }

    rusm.Demonitor(monRef)
    rusm.CancelTimer(timer)
    var reply map[string]any
    json.Unmarshal(raw, &reply)
    return reply["thumbnailUrl"].(string), nil
}
```

:::

## Demonitoring

Cancel a monitor with `demonitor(ref)` as soon as you no longer need it — typically the
moment the expected reply arrives. If you don't, and the worker exits later (e.g. after
finishing its job normally), a `__down` message will arrive in your mailbox unexpectedly
and confuse your receive loop.

---

Next: [Links & supervision](/build-an-app/links-and-supervision) — structured crash
propagation and automatic process restart.
