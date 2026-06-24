# Process management

Spawn, send, and receive are the basics. Beyond them, every RUSM process gets the full
Erlang **management toolkit** — a named registry, process groups, timers, introspection, and
orderly shutdown. All of it is backed by the Wasm-free `rusm-otp` core, so it works
identically whether a process body is native Rust or a sandboxed Wasm instance.

## Named registry

Register a process under a name so others can reach it without holding its pid:
`register(name)` / `whereis(name)`. The registry is a sharded `DashMap` (no global lock), so
lookups scale with cores. For names that resolve *across machines*, see
[distributed nodes](/deep-dive/distributed-nodes).

## Process groups (tags)

Where the registry maps **one name to one pid**, a process group (Erlang's `pg`) maps
**one tag to many pids** — and a process may hold many tags. A process joins a group by
tagging *itself* with `register_tag(tag)` (unprivileged, like `set_label`);
`whereis_tag(tag)` lists the live members; `kill_tag(tag)` terminates the whole group and
returns how many it killed; `unregister_tag(tag)` leaves. Memberships are released on exit
by the **same reaper that releases names**, so a group only ever reports live processes —
and tags add **zero hot-path cost** (an untagged process carries an empty list; the reaper
loops it only at exit).

`kill_tag` is the one privileged op — it terminates *other* processes — so it's gated by
the `process-control` capability exactly like `kill` (a sandboxed guest gets `0`). That
makes process groups the clean primitive for **scoped cancellation**: tag every process
that belongs to one unit of work — say `plan:<id>` for the agents a request spawned — and a
single `kill_tag("plan:<id>")` stops the whole unit immediately and authoritatively. The
application writes only two calls — self-tag on start, gated `kill_tag` to cancel — while
the platform owns the group registry, the reaping, and the gate. No cancel topics, no
polling, no pid bookkeeping in app code.

Same surface from all three guests: `register_tag`/`kill_tag`/`whereis_tag` in `rusm-rs`,
`Process.registerTag`/`killTag`/`whereisTag` in `rusm-ts`, `RegisterTag`/`KillTag`/`WhereisTag`
in `rusm-go` — backed, like everything here, by the Wasm-free `rusm-otp` core. This group
form is also RUSM's pub/sub primitive; see [broadcast to many](/build-an-app/broadcast-to-many).

## Timers

`send_after(to, delay_ms, msg)` schedules a message to arrive in `to`'s mailbox after
`delay_ms` milliseconds; `cancel_timer(handle)` aborts a pending one. Built on Tokio's
timer wheel, so millions of outstanding timers cost almost nothing.

## Introspection

`list()` enumerates live pids; `info(pid)` returns a process's links, monitors, names,
label, mailbox depth, and status; `set_label(..)` tags a process for the observer. This is
exactly what the [live-attach](/deep-dive/live-attach) REPL and the dashboard observer read —
nothing is hidden from you.

## Lifecycle: graceful shutdown vs kill

`shutdown` drains and stops a process in order; `kill` aborts it immediately via a `futures`
abort handle (no second signal channel — kill rides that handle, exit signals ride the
mailbox). Crashes and exits flow through [links & supervision](/deep-dive/links-and-supervision).

> Registry, timers, and graceful shutdown shipped in Phase 4; introspection, labels, and
> mailbox depth surface in the observer snapshot.
