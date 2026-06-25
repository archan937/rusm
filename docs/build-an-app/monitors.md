# Monitors

You spawned a worker, sent it a job, and you're waiting for the reply. What if the
worker crashes before it can respond? Without a safeguard, your `receive()` blocks
forever — a silent hang with no error.

A **monitor** closes that gap. Call `monitor(pid)` and the runtime watches that process
for you. The moment it exits — whether it finishes normally, crashes, or is killed — a
`__down` message lands in your mailbox. No polling, no watcher process of your own: you
just race your receive loop against it. If the result arrives, great; if `__down` arrives
first, the worker is gone and you handle the failure instead of hanging.

## Monitor vs. supervise

A monitor lets *you* react to a death while staying alive. When instead you want a dead
process **restarted automatically**, that's a supervisor's job.

| | Monitor | Supervisor |
|---|---|---|
| **You get** | a `__down` message — you decide what to do | an automatic restart, by strategy |
| **Stays alive?** | yes — the watcher keeps running | yes — the supervisor restarts the child |
| **Reach for it when** | a coordinator waits on a worker and must handle its loss | a long-lived child should self-heal on crash |

Monitors are the guest-facing primitive here; the bidirectional **links** that supervisors
are built on live in the runtime core — see [links & supervision](/deep-dive/links-and-supervision)
for that model, and [Links & supervision](/build-an-app/links-and-supervision) for the
in-guest `Supervisor`.

## Setting up a monitor

`monitor(pid)` starts watching a live process — including one you didn't spawn. It fires
**once**, when that process exits, delivering a `__down` to your mailbox. (There's no
`demonitor`: a monitor is a one-shot notification, not a subscription to manage.)

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const workerPid = Process.spawn("thumbnail-generator");

// Watch the worker — if it exits for any reason, we'll get a `__down` message.
Process.monitor(workerPid);
```

```rust [Rust]
let worker_pid = rusm_rs::spawn("thumbnail-generator").unwrap();

// Watch the worker:
rusm_rs::monitor(worker_pid);
```

```go [Go]
import rusm "github.com/archan937/rusm/packages/rusm-go"

workerPid, _ := rusm.Spawn("thumbnail-generator")

// Watch the worker:
rusm.Monitor(workerPid)
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
  Process.monitor(workerPid);

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

  return msg.thumbnailUrl as string;
}
```

```rust [Rust]
fn generate_thumbnail(image_url: &str) -> Option<String> {
    let worker_pid = rusm_rs::spawn("thumbnail-generator").ok()?;
    rusm_rs::monitor(worker_pid);

    // Send the job:
    let req = serde_json::json!({
        "replyTo": rusm_rs::me().to_string(),
        "url": image_url,
    });
    rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());

    // Receive: either the result or a __down message.
    let raw = rusm_rs::receive_bytes();

    // `down_pid` returns Some(pid) when `raw` is a __down signal.
    if let Some(dead_pid) = rusm_rs::down_pid(&raw) {
        eprintln!("thumbnail-generator crashed (pid {})", dead_pid);
        return None;
    }

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
    rusm.Monitor(workerPid)

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

    var reply map[string]any
    json.Unmarshal(raw, &reply)
    return reply["thumbnailUrl"].(string), nil
}
```

:::

## Combine with a timeout timer

For a complete safety net, combine a monitor with a [timer](/build-an-app/timers). You're
now protected against both a crash (the monitor's `__down`) and a hang (the timer's
`"timeout"` message):

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

async function generateThumbnailSafe(imageUrl: string): Promise<string | null> {
  const workerPid = Process.spawn("thumbnail-generator");
  Process.monitor(workerPid);
  Process.sendAfter(Process.self(), 5_000, "timeout");

  Process.send(workerPid, JSON.stringify({ replyTo: String(Process.self()), url: imageUrl }));

  const raw = await Process.receive();
  const msg = JSON.parse(new TextDecoder().decode(raw));

  if (msg === "timeout") {
    Process.kill(workerPid);   // still running, just too slow — stop it
    return null;
  }
  if (msg.__down) {
    return null;               // crashed before replying
  }

  return msg.thumbnailUrl as string;
}
```

```rust [Rust]
fn generate_thumbnail_safe(image_url: &str) -> Option<String> {
    let worker_pid = rusm_rs::spawn("thumbnail-generator").ok()?;
    rusm_rs::monitor(worker_pid);
    rusm_rs::send_after(rusm_rs::me(), 5_000, b"timeout");

    let req = serde_json::json!({ "replyTo": rusm_rs::me().to_string(), "url": image_url });
    rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());

    let raw = rusm_rs::receive_bytes();

    if raw == b"timeout" {
        rusm_rs::kill(worker_pid); // too slow — stop it
        return None;
    }
    if rusm_rs::down_pid(&raw).is_some() {
        return None;               // crashed before replying
    }

    let reply: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    Some(reply["thumbnailUrl"].as_str()?.to_string())
}
```

```go [Go]
func generateThumbnailSafe(imageUrl string) (string, error) {
    workerPid, _ := rusm.Spawn("thumbnail-generator")
    rusm.Monitor(workerPid)
    rusm.SendAfter(rusm.Self(), 5_000, []byte("timeout"))

    req, _ := json.Marshal(map[string]any{"replyTo": rusm.Self(), "url": imageUrl})
    rusm.Send(workerPid, req)

    raw := rusm.Receive()

    if string(raw) == "timeout" {
        rusm.Kill(workerPid) // too slow — stop it
        return "", fmt.Errorf("thumbnail-generator timed out")
    }
    if downPid, ok := rusm.DownPid(raw); ok {
        return "", fmt.Errorf("thumbnail-generator crashed: pid %s", downPid)
    }

    var reply map[string]any
    json.Unmarshal(raw, &reply)
    return reply["thumbnailUrl"].(string), nil
}
```

:::

## A monitor fires once

A monitor delivers exactly one `__down`, when the watched process exits — there's
nothing to cancel and nothing to clean up. One consequence worth knowing: if you monitor
a one-shot worker that **replies and then exits**, the `__down` still arrives, just after
the reply. A coordinator that handles many such calls should treat an unexpected `__down`
as "a process I was watching has exited" and ignore it if it's already done its job —
match on the message shape (a `__down` field, or `down_pid` / `DownPid` returning a pid)
and move on.

---

Next: [Links & supervision](/build-an-app/links-and-supervision) — automatic restart with
the in-guest `Supervisor`, and the crash-propagation model it's built on.
