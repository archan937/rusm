# Concept — links, traps & supervision ("let it crash")

Erlang's resilience doesn't come from preventing failures — it comes from
**isolating** them and **recovering** automatically. RUSM follows the same model.

## Traps become exits

A Wasm trap (panic, `unreachable`, out-of-bounds, exceeding a resource limit)
unwinds only that instance. The host catches the trap and records the process as
**crashed** rather than letting it take down anything else.

## Links and monitors

- **Link** (bidirectional): if either linked process dies abnormally, the other
  receives an exit signal — and, unless it traps exits, dies too. Used to bind the
  lifetimes of processes that only make sense together.
- **Monitor** (one-way): observe another process's exit without dying with it.

## Supervisors

A **supervisor** is just a process that spawns children, links/monitors them, and
**restarts** them according to a strategy (one-for-one, one-for-all, …) when they
crash. This is how a RUSM system heals itself: a bug crashes one request's
process, the supervisor restarts a clean one, the rest of the system never notices.

## In the dashboard

The fault-recovery scenario surfaces restarts/sec and recovery latency; the
observer shows `crashed` processes in red.

> Shipped in Phase 3 (task-panic isolation); upgraded to memory/trap-level
> isolation once the Wasmtime backend lands in Phase 6–7.
