# Glossary — Erlang/Elixir ↔ RUSM

Coming from the BEAM? RUSM's model is deliberately familiar. This table maps the
Erlang/Elixir concepts you already know onto their RUSM equivalents.

| Erlang/Elixir | RUSM | Notes |
| --- | --- | --- |
| process | a Wasm instance running as a Tokio task | own stack, heap, syscalls, permissions |
| scheduler | a Tokio worker thread (work-stealing) | M:N over a few OS threads |
| reduction counting | Wasmtime epoch interruption | forces fair yields, even in tight loops |
| mailbox | per-process async channel | host copies message bytes across memories |
| `send/2` | `send(pid, msg)` | fire-and-forget to a mailbox (`Process.send` / `send_bytes` / `Send`) |
| `receive` | `receive()` | suspends the process until a message arrives (`Process.receive` / `receive_bytes` / `Receive`) |
| link | bidirectional failure propagation (OTP core) | a crash signals linked peers; guests use `monitor` + an in-guest `Supervisor` |
| monitor | one-way failure notification | observe without dying together; a `__down` message, no polling |
| supervisor | a process that restarts crashing children | "let it crash" |
| `:global` | distributed registry | cluster-wide name → pid |
| `Node.connect/1`, epmd | QUIC + TLS node transport | secure node-to-node links |
| `iex --remsh` | `rusm attach <node>` | live REPL into a running node |
| `:observer` | the dashboard's observer view | live processes, schedulers, memory |
| BEAM | the RUSM runtime (Rust + Tokio + Wasmtime) | the host that runs everything |

Dashboard & benchmark terms — the wire between a node and the observer/REPL:

| Term | Meaning |
| --- | --- |
| frame | one sampled tick (throughput, latency, observer snapshot) sent to clients |
| scenario | a named benchmark (e.g. `connection-storm`) the node runs and streams; all scenarios now run on real engines |
| synthetic source | a deterministic generator producing scenario-shaped data per tick — the Phase-0 bootstrap, before the engines were real |
| detail toggle | switch for the costly per-instance observer table |
