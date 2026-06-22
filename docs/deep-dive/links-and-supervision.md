# Links & supervision

Erlang's resilience doesn't come from *preventing* failure — it comes from **isolating** it
and **recovering** from it automatically. "Let it crash": rather than defensively coding
around every error, you let a faulty process die and restart a clean one. RUSM follows the
same model, with Wasm making the isolation airtight.

## Traps become exits

A Wasm trap — a panic, `unreachable`, an out-of-bounds access, exceeding a resource limit —
unwinds only the instance that caused it. The host catches the trap and records that process
as **crashed**, rather than letting the failure escape. The blast radius of any bug is
exactly one process.

## Links and monitors

Once a process can crash cleanly, other processes need a way to find out:

- **Link** *(bidirectional)*: if either linked process dies abnormally, the other receives
  an exit signal — and, unless it traps exits, dies too. Use links to bind the lifetimes of
  processes that only make sense together, so a crash in one tears down the whole group.
- **Monitor** *(one-way)*: observe another process's exit *without* dying with it. Use
  monitors when you want to react to a death — log it, restart it — but outlive it.

## Supervisors

A **supervisor** is just a process that spawns children, links or monitors them, and
**restarts** them by a strategy when they crash — one-for-one (restart only the dead child),
one-for-all (restart them all), or rest-for-one. A windowed restart-intensity limit stops a
crash-looping child from spinning forever: too many restarts inside the window and the
supervisor gives up and escalates instead.

This is how a RUSM system heals itself: a bug crashes one request's process, its supervisor
restarts a clean one, and the rest of the system never notices. You build supervision trees
from inside a guest with the `Supervisor` helper — see
[coordinate & supervise](/build-an-app/coordinate-and-supervise).

## Seeing it work

The dashboard's **fault-recovery** scenario surfaces restarts/sec and recovery latency, and
the live observer shows `crashed` processes in red — failure and recovery, visible in real
time.

> Shipped in Phase 3 (task-panic isolation); upgraded to memory/trap-level isolation when
> the Wasmtime backend landed in Phases 6–7.
