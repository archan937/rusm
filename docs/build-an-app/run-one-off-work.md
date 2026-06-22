# Run one-off work

Sometimes you want to do a single job in its own process — generate a report, call an API,
crunch a batch — then have it go away. That's a **worker**: a component that is spawned, runs
once, and exits. (Contrast a [stateful service](/build-an-app/stateful-service), which is
*one long-lived* instance that stays and holds state. A worker is **ephemeral and one-shot**;
spawn as many as you like, each isolated.)

A worker exports a single entry — a `default` function in TS, `run` in Rust/Go — receives its
input, does the work, optionally replies, and returns (which ends the process).

::: code-group

```ts [TypeScript]
// components/report/index.ts — receive one job, do it, reply, return (process exits).
import { Process } from "rusm-ts";

export default async function () {
  const job = JSON.parse(await Process.receiveText()); // blocks; the fiber parks
  const result = buildReport(job);                     // your work
  Process.send(job.replyTo, JSON.stringify(result));   // reply to whoever asked
}
```

```rust [Rust]
// components/report/src/lib.rs
#[rusm_rs::main]
fn run() {
    let job: Job = rusm_rs::receive().unwrap(); // blocks; the fiber parks
    let result = build_report(&job);
    rusm_rs::send(job.reply_to, &result).ok();  // reply to the caller
    // returning ends the process — it exits Normal
}
```

```go [Go]
// components/report/main.go
func run() {
	var job Job
	rusm.Receive(&job)                 // blocks; the fiber parks
	result := buildReport(job)
	rusm.Send(job.ReplyTo, result)     // reply to the caller
	// run returns → the process exits Normal
}
```

:::

Declare it in `rusm.toml` like any component (no `resident` — it isn't long-lived):

```toml
[components.report]
capability = "sandboxed"
```

A caller spawns one per job — `spawn("report")` then send it the work (the `allow-spawn`
capability is required; see [Call another component](/build-an-app/call-another-component) for
the typed-client version). Each worker is a fresh sandboxed process, so ten reports run as ten
isolated processes; if one panics it exits `Crashed` and the others are untouched.

## Worker or service?

| | **Worker** (this page) | **[Stateful service](/build-an-app/stateful-service)** |
| --- | --- | --- |
| Lifetime | spawned per job, **exits when done** | **long-lived**, boot-spawned + supervised |
| Count | many, one per job | one shared instance, found by name |
| State | none (gone after the job) | holds state in memory across calls |
| Declared | a plain `[components.<name>]` | `[components.<name>]` with `resident = true` |
| Use it for | background tasks, fan-out, isolation per job | a registry, a counter, a cache, a broker |
