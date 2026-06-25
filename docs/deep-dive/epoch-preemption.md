# Epoch preemption

Tokio scheduling is **cooperative** — a task only yields when it `await`s. That's a problem
for untrusted guests: a tight `loop {}` with no host calls would never yield, hogging a
worker thread and starving every other process. The BEAM solves this by counting
"reductions" and preempting. RUSM solves it with **Wasmtime epoch interruption** — fair
scheduling even when a guest never cooperates.

## How it works

Wasmtime compiles guest code with periodic **epoch checks**. A **dedicated background
thread** bumps a global epoch counter on a fixed cadence (~10 ms) — on its own thread so a
CPU-pinned guest can't starve the bump itself; when a running guest crosses an epoch
boundary, Wasmtime interrupts it. RUSM yields the fiber back to the scheduler (see
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)) and resumes it later.
The upshot: even an infinite loop yields its fair share of the CPU.

## Why epochs, not fuel

Wasmtime also offers **fuel** — decrement a counter per instruction — but epoch interruption
is far cheaper at runtime: one periodic counter check versus per-instruction accounting. On
a hot path juggling hundreds of thousands of processes, that difference matters.

## The test that proves it

Phase 6 ships a **fairness** test: spawn a process running an infinite loop alongside others
that must keep making progress — still receiving messages, say. With epoch interruption on,
the bystanders are never starved. The dashboard's fairness scenario shows the same thing
live, with bystanders holding 50M+ ops/sec under a tight-loop spinner.

> Shipped in Phase 6 (requires the Wasmtime backend).
