# Concept — cross-process byte streams

Messages are whole values: you `send` a chunk, the receiver gets that chunk. But
some work is a *flow* — an HTTP body, an SSE feed, a file being piped — where the
producer keeps emitting and the consumer keeps reading, and neither should have to
buffer the whole thing in memory. RUSM models that as a **byte stream** between
processes.

## When you actually use it

A byte stream is a **low-level primitive**. Its heaviest user is RUSM's own **serving
layer**: an SSE or streaming HTTP/WS body rides a back-pressured byte stream, so a slow
client throttles the producer instead of forcing the server to buffer a whole response —
and you get that for free by writing a normal SSE/WS handler, never touching the stream API.
An **application component** reaches for the raw producer/consumer API directly only to pipe
a large or open-ended payload from one component to *another* — a proxy forwarding an upload,
a transcoding stage, an LLM-token relay. For everything else, component-to-component talks in
messages (`send`/`receive`) and client streaming goes through the serving API.

## A bounded channel, the actor way

A stream is a **bounded Tokio channel** of byte chunks (`StreamHandle` in the
Wasm-free `rusm-otp` core). The read end travels in a message —
`Received::Stream` — moving ownership to the recipient exactly like any other
message. So streams are pure actor composition: no shared memory, no new wiring.

## Back-pressure for free

Because the channel is bounded, a slow reader automatically slows the writer: the
writer's `stream_write` simply **parks its fiber** until there's room (see
[fibers & blocking→async](./fibers-and-blocking-to-async.md)). No busy-polling, no
unbounded memory growth — the safety property that lets a component stream an
HTTP/SSE/WS body without falling over.

## From guests

A Wasm guest drives streams through the actor ABI — both **components** (the
`rusm:runtime` WIT world: `stream-open`/`write`/`close`/`accept`/`read`) and
**wasip1 core modules** (the raw `rusm::*` ABI). `stream-open(to)` hands the read
end to another process and keeps the write end; the write/read ops move chunks. The
two byte copies — *out of* the producer's sandboxed memory and *into* the
consumer's — are the price of true isolation; everything between is a zero-copy
channel hand-off. The **stream-pipe** benchmark sustains multiple GB/s across
producer→consumer pairs.

> Shipped in Phase 7 (core `StreamHandle` in Phase 2; the guest ABI in Phase 7).
