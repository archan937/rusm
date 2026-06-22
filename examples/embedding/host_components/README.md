# host_components

**Hosting WASM components on RUSM's actor model** — the heart of Phase 7.

Loads real WASM **components** and runs them as isolated, observable, sandboxed
processes:

1. **Host a component as a process** — `compile_component` → `prepare_component`
   (imports + entry export resolved once) → `spawn_component`. Then introspect it
   with `Process.info`/`Process.list`, exactly like a native process.
2. **Capabilities (default-deny)** — the same "hungry" component is spawned under
   two profiles: a 64 KiB memory cap (its growth is denied → it traps → `Crashed`)
   and an 8 MiB cap (it finishes `Normal`). Sandboxing is per-process.

Run it:

```sh
cargo run -p rusm-bench --example host_components
```

Expected (abridged):

```
hosted a component as process Pid(0)
  Process.info -> links=0, mailbox_depth=0
  Process.list -> [Pid(0)]
  it ran and was reaped; live processes now: 0

hungry component, 64 KiB cap  -> Crashed
hungry component, 8 MiB cap   -> Normal

Same component, two capability profiles — sandboxed by construction.
```

A guest written in Rust (`wit-bindgen`) or TS (Bun + rquickjs) would additionally
import the `rusm:runtime` actor world to `spawn`/`send`/`receive`/`register` — see
the [host ABI](../../../docs/deep-dive/host-abi-reference.md) and the [Phase 7
doc](../../../docs/phases/phase-07-components.md). To run a whole app of components,
see the app model: `rusm.toml [components.<name>]` + `rusm build` / `rusm dev`.
