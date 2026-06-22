# Phase 2 — mailboxes & message passing

**Goal:** turn processes into *actors* — each gets a private mailbox, and they
talk only by sending messages. Adds `send`, `recv`, and selective `recv_match`.
**Graduates:** the **ping-pong** scenario to live data.

## Why this matters

Isolation without communication is useless; shared memory without isolation is
unsafe. The actor model's answer — copy a message into the recipient's mailbox,
never share — is what makes "let it crash" and (later) cross-instance Wasm
messaging work without data races.

## What we built (TDD throughout)

1. **One mailbox per process** — a Tokio `mpsc::unbounded` receiver lives in the
   process's `Context`; the sender half lives in its table entry.
2. **`Received` enum** — a mailbox carries more than user data:
   `Message(Vec<u8>)`, `Down { reference, pid, reason }`, and `Exit { from, reason }`.
   One channel, one ordering, for messages *and* signals (the monitor/link
   payloads land here in [Phase 3](./phase-03-supervision.md)).
3. **`send(pid, msg) -> bool`** — enqueues into the target mailbox; returns
   `false` for a dead pid (no panic, Erlang-style "send never fails").
4. **`recv().await`** — suspends the process until a message arrives, yielding
   the Tokio worker while parked (the basis of cheap massive concurrency).
5. **Selective receive — `recv_match(pred)`** — scans the mailbox for the first
   message matching a predicate, stashing non-matches in a `saved` `VecDeque` and
   replaying them first on the next receive. This is Erlang's selective receive,
   preserving arrival order for the messages left behind.

## How a developer uses it

```rust
// Inside a process body: ctx is the process's Context.
let msg = ctx.recv().await.message();          // wait for the next user message
runtime.send(peer, b"ping".to_vec());          // fire a message at another pid

// Selective receive: take the first reply, leave everything else queued in order.
let reply = ctx.recv_match(|m| m.message()
    .map_or(false, |b| b.starts_with(b"reply:"))).await;
```

## Design notes

- **One channel, not two.** Messages and exit/down signals share a single
  ordered mailbox, so a process sees a single, well-defined event stream — and we
  avoid the per-process two-channel overhead.
- **`Vec<u8>` payloads.** The core stays serialization-agnostic; structure is the
  guest's concern (and the Wasm ABI's in Phase 6).

## Concepts introduced

- **Copying across isolated memories** and selective receive — see
  [message passing](/deep-dive/message-passing).

## Play with it

```sh
cargo run -p rusm-bench -- run ping-pong 5    # real round-trips, ~21M msgs/sec
```

## Verification

`cargo test -p rusm-otp` green (FIFO order, send-to-dead, selective-receive
ordering, park/wake); ping-pong shows ~21M msgs/sec, round-trip p50 <1 µs.

## Next

[Phase 3](./phase-03-supervision.md): **links, monitors, supervision** — exit
reasons, cascades, and "let it crash".
