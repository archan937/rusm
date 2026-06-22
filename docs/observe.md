# Observe a running node

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
it's built. See [the benchmark dashboard](./03-benchmark-dashboard) for the full
walkthrough, and [live attach](./concepts/live-attach) for the attach protocol.
