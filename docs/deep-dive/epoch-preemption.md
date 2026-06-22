# Concept — fair preemption via epoch interruption

Tokio scheduling is **cooperative**: a task only yields when it `await`s. A guest
running a tight `loop {}` with no host calls would never yield and would hog a
worker thread — starving other processes. The BEAM avoids this by counting
"reductions" and preempting. RUSM uses **Wasmtime epoch interruption**.

## How it works

Wasmtime can compile guest code with periodic **epoch checks**. A background
timer bumps a global epoch counter on a fixed cadence. When a running guest
crosses an epoch boundary, Wasmtime interrupts it — RUSM yields the fiber back to
the scheduler (see [fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)),
then resumes it later. The result: even an infinite loop yields fairly.

## Why epochs (not fuel)

Wasmtime also offers *fuel* (decrement per instruction), but epoch interruption
is cheaper at runtime — a single periodic counter check rather than per-step
accounting — which suits a hot path with hundreds of thousands of processes.

## The test that proves it

Phase 6 ships a fairness test: spawn a process with an infinite loop alongside
others that must keep making progress (e.g. still receive messages). With epoch
interruption on, the bystanders are never starved.

> Shipped in Phase 6 (needs the Wasmtime backend).
