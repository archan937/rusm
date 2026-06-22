# Concept — message passing across isolated memories

Processes share nothing, so they communicate by **copying** bytes through the
host. Each process has a mailbox (an async channel). Sending is fire-and-forget;
receiving suspends the process until a message arrives.

## The flow

1. Sender builds a message in its own linear memory and calls
   `rusm::message::send(pid)`.
2. The host **copies** the message bytes out of the sender's memory and pushes
   them onto the target's mailbox (a Tokio channel). No memory is shared.
3. The target calls `rusm::message::receive()`, which awaits the mailbox; the
   host copies the bytes **into** the target's memory.

## Why copy, not share

Sharing memory between instances would break isolation — the whole point of the
model. Copying keeps each crash and each permission boundary local. Messages are
ordinary serialized data (the `rusm-rs` and `rusm-ts` guest crates send serde
JSON — one wire shared across Rust and TS guests).

## Receive suspends, it doesn't spin

`receive()` is an async host call: an empty mailbox parks the Tokio task (see
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)), so a million
waiting processes cost almost nothing.

> Shipped in Phase 2 (opt-in mailbox depth surfaces in the observer snapshot).
