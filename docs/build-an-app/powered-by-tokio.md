# Powered by Tokio

Before diving into processes, mailboxes, and supervision, it's worth understanding what
runs underneath all of it — because it's a big part of why RUSM's process model is
trustworthy.

## What is Tokio?

[Tokio](https://tokio.rs) is the async runtime for Rust. It schedules async tasks,
drives I/O (TCP, timers, file I/O) without blocking threads, and ships a work-stealing
thread pool that keeps every CPU core busy. It is **not** something RUSM built — it is
the most widely used async runtime in the Rust ecosystem, battle-proven in production at
**Amazon, Discord, Cloudflare, Fly.io, and hundreds of other companies** at massive
scale. When you run a RUSM node, Tokio is the engine that every single process runs on.

## Why it matters for you

When you spawn a RUSM process, you're creating a **Tokio task** — a lightweight async
unit of execution. Tokio tasks cost a few hundred bytes of memory and a handful of
nanoseconds to create. You can have tens of thousands of them on a single machine without
breaking a sweat.

Tokio's scheduler does three things that matter deeply for RUSM:

**Work-stealing.** Tasks are distributed across a thread pool that matches your CPU
core count. An idle worker thread steals tasks from busy ones. The result: all cores
stay utilized, and no single slow process can hog a thread and starve others. Fairness
is structural, not something RUSM had to implement.

**Non-blocking I/O.** When a RUSM process calls `receive()` and there's nothing in its
mailbox, it doesn't block a thread — it suspends its task and yields the thread to
other work. The thread is free to run thousands of other processes while this one waits.
This is how RUSM achieves **~21M messages/sec** with **p50 round-trip latency under
1µs**: the overhead of a context switch is a task yield, not a kernel thread park.

**Cooperative preemption.** RUSM adds epoch-based preemption on top of Tokio so that
a CPU-bound WebAssembly guest (an infinite loop, heavy computation) doesn't monopolize
a thread. The runtime periodically checks whether a Wasm instance has run too long and
yields it. You get cooperative multitasking with a safety net — long-running guests
don't starve their neighbors.

## One process = one Tokio task

The mapping is exact and intentional:

```
rusm process  ─────►  Tokio task
                       ├── one Wasm instance (for Wasm guests)
                       ├── one mailbox
                       └── one abort handle (for kill)
```

Every process is exactly one Tokio task. No thread per process (that would cap you at
thousands, not millions). No green thread runtime RUSM invented itself (that would be
unproven). Just the Tokio task abstraction, which has been hardened over years of
production use.

`kill` cancels the Tokio task via its **abort handle** — one field, one call, no second
signal channel. Exit signals and monitor notifications arrive in the mailbox as ordinary
messages — the same queue, the same receive loop. Nothing special to wire up.

## The numbers

Measured on a laptop under everyday load, using RUSM's published benchmarks:

| Benchmark | Number |
|---|---|
| Native process spawns/sec (Wasm-free OTP core) | ~2.4M |
| Sandboxed Wasm component spawns/sec | ~440k |
| Messages/sec (round-trip, two processes) | ~21M |
| Round-trip latency p50 | <1µs |
| Concurrent connections (connection-storm bench) | thousands |

These are not theoretical ceilings — they are measured numbers from `rusm-bench` on
real hardware. The limit in practice is your machine's memory and OS file descriptor
count. RUSM imposes no fixed process cap.

## What this means in practice

You don't need to think about thread pools, connection pools, or worker queues. You
spawn a process for each unit of work — one per HTTP request, one per WebSocket
connection, one per background job — and Tokio handles the scheduling. If a process
blocks waiting for a message, Tokio runs other processes on that thread. If a process
crashes, nothing else is affected. The runtime has done this for you.

This is why RUSM can make the claim: **write blocking code, the runtime makes it async**.
You write a straightforward `receive()` call; under the hood it's a Tokio task yield.
The transformation is transparent and zero-cost.

---

Next: [What is a process?](/build-an-app/what-is-a-process) — how processes are modelled
on top of this foundation.
