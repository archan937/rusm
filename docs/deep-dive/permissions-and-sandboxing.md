# Concept — per-process permissions & sandboxing

Because every process is an isolated Wasm instance, RUSM can grant each one a
**precise, individual** set of capabilities — finer-grained than Go or Erlang,
where any goroutine/process can touch the whole machine.

## What can be scoped per process

- **Filesystem**: which directories (if any) the process may open — via WASI
  preopens. A process with no preopens cannot touch the disk at all.
- **Network**: whether outbound/inbound sockets are allowed.
- **Memory**: a hard ceiling via Wasmtime store limits; exceeding it traps *that*
  process only (see [links & supervision](/deep-dive/links-and-supervision)).
- **Process control**: whether the process may `kill`/`list`/`inspect` *other*
  processes via the actor ABI — default-deny, so a sandboxed process manages only
  itself.
- **Spawn**: whether the process may **spawn other components by name** via the
  actor ABI — default-deny. The `allow-spawn` capability gates *who* may spawn; a
  **node-registered** component (a `[components.<name>]` / serve entry) then runs under
  **its own manifest-declared profile** — the operator's explicit per-component
  policy, so what the manifest declares is what runs, whoever spawns it. An *ad-hoc*
  registration with no declared profile inherits the spawner's caps. Either way a
  guest can never fabricate capabilities the operator didn't grant.
- **Storage**: whether the process may use the node's **durable key-value store**
  (the `kv-*` ABI, backed by `rusm-kv`/redb) — default-deny. A sandboxed process
  has no persistence; the node must also have a `store` configured.
- **Host functions**: only the `rusm::*` imports the process was linked with are
  callable.

## Default-deny profiles

A process gets **nothing** unless granted. Named profiles (`caps.rs`) bundle
sensible defaults, and a per-spawn `Capabilities` builder overrides them:

- **`Sandboxed`** — CPU + a bounded heap only: no fs, net, env, stdio, control, or spawn.
- **`NetworkClient`** — sandboxed plus outbound network.
- **`Trusted`** — inherits stdio, allows network, process control, spawn, storage, a large heap.

Grants map onto **standard WASI** (`wasi:cli/environment`, `wasi:filesystem`,
`wasi:sockets`) plus a `StoreLimiter` memory cap — no wasmCloud-style
`wasi:config/store`.

### Custom profiles in the manifest

An app author isn't limited to the three built-ins. `rusm.toml` accepts custom
`[capabilities.<name>]` profiles — like Cargo's `[profile.<name>]`: each
`inherits` a built-in base (default `sandboxed`) and overrides only the grants it
sets. A component selects one with `capability = "<name>"`; an unknown name falls
back to the default-deny `sandboxed`.

```toml
[capabilities.agent]
inherits = "network-client"
allow-spawn = true
max-memory-mb = 256
env = ["OPENAI_API_KEY"]
preopen = [{ host = "./data", guest = "/data", read-only = false }]
```

## Why it matters

You can run untrusted or risky code — even a C library compiled to Wasm with a
latent bug — knowing a crash or exploit is confined to that one process and its
granted resources. This is the security half of "isolation"; fault tolerance is
the other half.

## The test that proves it

Tests cover a memory-limit breach trapping a single process (both the component
and core-module paths) without disturbing the rest, and an unbuildable grant
(a preopen of a missing path) crashing only that process before it runs.

> Shipped in Phase 7.
