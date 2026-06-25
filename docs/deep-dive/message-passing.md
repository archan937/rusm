# Message passing

Processes in RUSM **share nothing** — separate memories, separate permissions. So they
communicate the only safe way: by **copying** bytes through the host. Each process owns a
mailbox (an async channel); sending is fire-and-forget, and receiving suspends the receiver
until a message arrives. It's Erlang's model, made airtight by Wasm's memory boundary.

## The flow of a message

1. The sender builds a message in its own linear memory and calls the host **`send`** op
   (`send: func(to: pid, message: list<u8>)` in the `rusm:runtime` actor interface; raw
   core modules call the equivalent `rusm.send`).
2. The host **copies** those bytes out of the sender's memory and pushes them onto the
   target's mailbox (a Tokio channel). No memory is ever shared.
3. The target calls **`receive`** (`receive: func() -> list<u8>`), which awaits the
   mailbox; the host copies the bytes **into** the target's memory.

## Why copy, not share

Sharing memory between instances would break isolation — the whole point of the model. A
shared buffer means one process's bug, or one permission boundary, can reach into another's
memory. Copying keeps every crash and every capability boundary strictly local.

The messages themselves are ordinary serialized data: the `rusm-rs`, `rusm-ts`, and
`rusm-go` guest SDKs all send serde JSON over **one shared wire**, so a Rust, a TypeScript,
and a Go guest can message each other transparently.

## Receiving suspends — it never spins

`receive()` is an async host call. An empty mailbox **parks** the Tokio task (see
[fibers & blocking→async](/deep-dive/fibers-and-blocking-to-async)) rather than
busy-waiting, so a million processes all blocked on `receive` cost almost nothing — they're
just idle tasks. That's exactly what makes "a process per request, per connection, per unit
of work" affordable.

> Shipped in Phase 2. (Opt-in mailbox depth surfaces in the observer snapshot.)
