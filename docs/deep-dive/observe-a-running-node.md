# Observe a running node

Watch a live node from the outside — its process count and per-process table, streaming
as it runs. Two front-ends sit on the same [attach protocol](/deep-dive/live-attach): the
`rusm attach` terminal REPL for any app, and the React benchmark dashboard for the repo.

**Your app** — start it as an attachable node, then attach a REPL to watch its
live processes:

```sh
rusm node start           # hosts your rusm.toml components + a live attach endpoint
rusm attach               # stream the live process table; `detail off` for just counts
```

**The benchmark dashboard** — the visual observer + scenario runner (repo-only):

```sh
make dashboard            # the benchmark node + the React dashboard ("the money")
# or run them separately:  make node   then   make ui
```

The dashboard's **Observer** shows the live process count and per-tick activity;
each scenario panel also unfolds its real engine source so you can see exactly how
it's built.

## What you see

Both front-ends stream the same shape: a **live process count** and a **per-process
table** — each row is one live process with its label, any registry names it holds, its
mailbox depth, and its links. In the REPL it looks like this (illustrative):

```
attached — type `help` for commands
processes: 1,284
  PID    LABEL          REGISTERED   MAILBOX   LINKS
  42     api#req         —            0         0
  43     store           store        2         1
  44     chat#conn       room:general 0         0
  …
> detail off                # just the live count, no per-process table
processes: 1,284
```

The view is a **periodic aggregated snapshot** (10–60 Hz), not an event per operation, so
watching a node barely costs it anything — the per-process table is the only expensive part,
which is why `detail off` exists for clean high-rate runs. See
[observability must stay cheap](/about/benchmark-dashboard-and-observer#observability-must-stay-cheap).

## Evaluate JavaScript live

`rusm attach` is also a **JavaScript shell** into the running node — RUSM's
`iex --remsh`. Any line that isn't a built-in command (`detail`, `help`, `quit`) is
evaluated against the live node, with the full `Process` API in scope and **bindings that
persist across lines**:

```
> Process.list().length
3
> p = Process.whereis("store")
43
> Process.send(p, JSON.stringify({ op: "ping" }))
> const before = Process.list().length
> Process.kill(p); Process.list().length
2
```

Each line runs inside a sandboxed REPL component (one per connection, `Trusted`), so the
shell can inspect, message, and kill any process — `whereis` / `list` / `send` / `kill` /
`killTag` all work, across Rust, TypeScript, and Go processes alike. `await` works at the
top level, console output is echoed back, and a thrown error is reported without dropping
your session.

**Eval is local-only.** The node accepts it only from a loopback client — the interim
security boundary until the attach channel is authenticated. A remote `rusm attach` can
still observe (the process table streams), but its eval lines are refused.

See [the benchmark dashboard](/about/benchmark-dashboard-and-observer) for the full
walkthrough, and [live attach](/deep-dive/live-attach) for the attach protocol.
