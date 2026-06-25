# Phase 2 — Mailboxes & message passing

Isolation without communication is useless — Phase 2 gives every process a private mailbox and one rule: the only way to share data is to copy a message into someone else's queue.

## Why this phase

Phase 1 proved processes can spawn and die cheaply. But isolated processes that can't talk to each other are just threads with extra steps. The actor model's core insight — copy a message into the recipient's mailbox, never share memory — is what makes "let it crash" work, what makes cross-Wasm-instance messaging possible without data races, and what keeps the programming model the same whether processes live on one machine or across a cluster.

Phase 2 draws that line. After it, every interaction between processes goes through the mailbox. There are no other coordination primitives in the model.

## What shipped

1. **One mailbox per process** — a Tokio `mpsc::unbounded` receiver lives in the process's `Context`; the sender half lives in the table entry. No allocation on send; no lock on receive.
2. **`Received` enum** — a mailbox carries more than user bytes: `Message(Vec<u8>)`, `Down { reference, pid, reason }`, and `Exit { from, reason }`. One channel, one ordering, for messages *and* signals. The supervision primitives from Phase 3 land here — already designed in.
3. **`send(pid, msg) -> bool`** — enqueues into the target mailbox; returns `false` for a dead pid. Send never panics, never throws — Erlang semantics.
4. **`recv().await`** — suspends the process until a message arrives, yielding the Tokio worker while parked. The foundation of cheap massive concurrency: a parked process costs nothing.
5. **Selective receive — `recv_match(pred)`** — scans the mailbox for the first message matching a predicate, stashing non-matches in a `saved` `VecDeque` and replaying them on the next receive. Erlang's selective receive, with full arrival-order preservation for messages left behind.

## Design highlights

- **One channel, not two.** Messages and exit/down signals share a single ordered mailbox, so a process sees one well-defined event stream. No separate signal channel, no per-process two-channel overhead — and signals can't reorder relative to messages.
- **`Vec<u8>` payloads — serialization-agnostic core.** The runtime doesn't care what's in a message. Structure is the guest's concern (and the Wasm ABI's in Phase 6). JSON, msgpack, raw bytes — all equally supported.
- **Send never fails loudly.** Sending to a dead pid returns `false`. The caller decides what to do; the runtime doesn't crash. This is one of the properties that makes "let it crash" safe — a crash in one process doesn't propagate through send calls.
- **Selective receive preserves queue order.** Non-matching messages are stashed and replayed first on the next receive. A message skipped today isn't lost — it arrives in its original position relative to other non-matched messages.

## What this unlocks

Processes are now actors. You can build request/reply patterns, pipelines, broadcast fans, and stateful services — all over message passing, all without shared memory. The ping-pong scenario goes live here and demonstrates ~21M round-trip messages/sec at p50 <1µs, on one machine, in process pairs.

Phase 3 adds links and monitors — but those are just new variants of `Received`. The mailbox was already designed to carry them.

## Try it

```sh
cargo run -p rusm-bench -- run ping-pong 5    # ~21M msgs/sec, round-trip p50 <1µs
```

## Status

Phase complete. ~21M messages/sec, round-trip p50 <1µs. Ping-pong is live in the dashboard.

---

*Next: [Phase 3](./phase-03-supervision.md) — links, monitors, supervision: failures propagate, supervisors restart, fault-recovery goes live at ~285k restarts/sec.*
