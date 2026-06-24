# Timers

A timer schedules a message to arrive in a process's mailbox after a delay. That's it.
No callbacks, no special event loop integration — the message lands in the same mailbox
as every other message, and you handle it in the same receive loop.

This design means timers are composable with everything else: a timeout, a retry, a
heartbeat — each one is just a delayed send to yourself (or any other process).

## send_after

`send_after(to, delayMs, message)` schedules `message` to arrive in `to`'s mailbox
after `delayMs` milliseconds. It returns a **timer handle** you can use to cancel the
timer before it fires.

The argument order — target pid first, delay second — is consistent with all other
RUSM process-targeting operations (`send`, `kill`, `monitor`).

The typical pattern is to schedule a timeout for yourself, then race your receive loop
against it:

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const TIMEOUT_MS = 5_000;

// Schedule a "timeout" message to ourselves in 5 seconds.
const timer = Process.sendAfter(Process.self(), TIMEOUT_MS, "timeout");

// Ask a worker to do something:
const workerPid = Process.spawn("slow-report-generator");
Process.send(workerPid, JSON.stringify({ replyTo: String(Process.self()), reportId: "Q4-2025" }));

// Wait for the reply — or the timeout, whichever comes first.
const raw = await Process.receiveText();

if (raw === "timeout") {
  console.error("report generation timed out after 5s");
} else {
  // Got the report. Cancel the timer so the "timeout" message never arrives.
  Process.cancelTimer(timer);
  const report = JSON.parse(raw);
  console.log("report ready:", report.title);
}
```

```rust [Rust]
const TIMEOUT_MS: u64 = 5_000;

// Schedule a "timeout" message to ourselves in 5 seconds.
let timer = rusm_rs::send_after(rusm_rs::me(), TIMEOUT_MS, b"timeout");

// Ask a worker to do something:
let worker_pid = rusm_rs::spawn("slow-report-generator").unwrap();
let req = serde_json::json!({ "replyTo": rusm_rs::me().to_string(), "reportId": "Q4-2025" });
rusm_rs::send_bytes(worker_pid, req.to_string().as_bytes());

// Race: reply or timeout.
let raw = rusm_rs::receive_bytes();
if raw == b"timeout" {
    eprintln!("report generation timed out after 5s");
} else {
    rusm_rs::cancel_timer(timer); // cancel before the timeout message arrives late
    let report: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    println!("report ready: {}", report["title"]);
}
```

```go [Go]
import (
    "encoding/json"
    "fmt"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

const timeoutMs = 5_000

// Schedule a "timeout" message to ourselves.
timer := rusm.SendAfter(rusm.Self(), timeoutMs, []byte("timeout"))

// Ask a worker to do something:
workerPid, _ := rusm.Spawn("slow-report-generator")
req, _ := json.Marshal(map[string]any{"replyTo": rusm.Self(), "reportId": "Q4-2025"})
rusm.Send(workerPid, req)

// Race: reply or timeout.
raw := rusm.ReceiveBytes()
if string(raw) == "timeout" {
    fmt.Println("report generation timed out after 5s")
} else {
    rusm.CancelTimer(timer)
    var report map[string]any
    json.Unmarshal(raw, &report)
    fmt.Println("report ready:", report["title"])
}
```

:::

## cancel_timer

Call `cancel_timer(handle)` to abort a pending timer. If the timer has already fired
(the message is already in the mailbox), `cancel_timer` is a no-op — the message is
already there and you'll see it on the next `receive`. That's fine: just check for it in
your receive loop and discard it.

::: code-group

```ts [TypeScript]
const timer = Process.sendAfter(Process.self(), 10_000, "cleanup");

// ... do work that finishes early ...

// Work finished before the 10s cleanup timer — cancel it.
Process.cancelTimer(timer);
```

```rust [Rust]
let timer = rusm_rs::send_after(rusm_rs::me(), 10_000, b"cleanup");

// ... work finished early ...

rusm_rs::cancel_timer(timer);
```

```go [Go]
timer := rusm.SendAfter(rusm.Self(), 10_000, []byte("cleanup"))

// ... work finished early ...

rusm.CancelTimer(timer)
```

:::

## Heartbeat pattern

A resident service can drive periodic work by sending itself a message at a fixed
interval. The trick: schedule the **next** heartbeat from *within* the heartbeat handler.
That way the service is self-sustaining — no external scheduler, no cron job.

::: code-group

```ts [TypeScript]
import { Process } from "rusm-ts";

const HEARTBEAT_MS = 30_000; // every 30 seconds

Process.register("cache-service");

// Kick off the first heartbeat immediately.
Process.sendAfter(Process.self(), HEARTBEAT_MS, "heartbeat");

while (true) {
  const msg = await Process.receiveText();

  if (msg === "heartbeat") {
    // Do periodic work — evict stale entries, flush metrics, etc.
    evictStaleEntries();
    // Schedule the next heartbeat.
    Process.sendAfter(Process.self(), HEARTBEAT_MS, "heartbeat");
    continue;
  }

  // Handle normal service requests:
  const req = JSON.parse(msg);
  // ...
}

function evictStaleEntries() { /* ... */ }
```

```rust [Rust]
const HEARTBEAT_MS: u64 = 30_000;

rusm_rs::register("cache-service");
rusm_rs::send_after(rusm_rs::me(), HEARTBEAT_MS, b"heartbeat");

loop {
    let raw = rusm_rs::receive_bytes();

    if raw == b"heartbeat" {
        evict_stale_entries();
        rusm_rs::send_after(rusm_rs::me(), HEARTBEAT_MS, b"heartbeat");
        continue;
    }

    // Handle normal service requests:
    let req: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    // ...
}

fn evict_stale_entries() { /* ... */ }
```

```go [Go]
import (
    "encoding/json"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

const heartbeatMs = 30_000

rusm.Register("cache-service")
rusm.SendAfter(rusm.Self(), heartbeatMs, []byte("heartbeat"))

for {
    raw := rusm.ReceiveBytes()

    if string(raw) == "heartbeat" {
        evictStaleEntries()
        rusm.SendAfter(rusm.Self(), heartbeatMs, []byte("heartbeat"))
        continue
    }

    // Handle normal service requests:
    var req map[string]any
    json.Unmarshal(raw, &req)
    // ...
}

func evictStaleEntries() { /* ... */ }
```

:::

## Retry with backoff

Schedule a retry after a delay by sending yourself the original request again. No retry
framework needed — it's just a delayed self-send:

::: code-group

```ts [TypeScript]
async function fetchWithRetry(url: string, attempt: number): Promise<string> {
  try {
    const res = await fetch(url);
    return await res.text();
  } catch {
    if (attempt >= 3) throw new Error(`failed after 3 attempts: ${url}`);
    const delayMs = 500 * Math.pow(2, attempt); // 500ms, 1s, 2s
    // Schedule a retry message to ourselves after the backoff delay.
    Process.sendAfter(Process.self(), delayMs, JSON.stringify({ retry: url, attempt: attempt + 1 }));
    return ""; // caller will receive the retry message and call again
  }
}
```

```rust [Rust]
fn schedule_retry(url: &str, attempt: u32) {
    if attempt >= 3 {
        log::error!("failed after 3 attempts: {}", url);
        return;
    }
    let delay_ms = 500 * 2u64.pow(attempt); // 500ms, 1s, 2s
    let msg = serde_json::json!({ "retry": url, "attempt": attempt + 1 });
    rusm_rs::send_after(rusm_rs::me(), delay_ms, msg.to_string().as_bytes());
}
```

```go [Go]
import (
    "encoding/json"
    "math"
    rusm "github.com/archan937/rusm/packages/rusm-go"
)

func scheduleRetry(url string, attempt int) {
    if attempt >= 3 {
        return
    }
    delayMs := int64(500 * math.Pow(2, float64(attempt))) // 500ms, 1s, 2s
    msg, _ := json.Marshal(map[string]any{"retry": url, "attempt": attempt + 1})
    rusm.SendAfter(rusm.Self(), uint64(delayMs), msg)
}
```

:::

---

Next: [Monitors](/build-an-app/monitors) — get notified when another process exits,
without crashing yourself.
