# Glossary — Erlang/Elixir ↔ RUSM

| Erlang/Elixir | RUSM | Notes |
| --- | --- | --- |
| process | a Wasm instance running as a Tokio task | own stack, heap, syscalls, permissions |
| scheduler | a Tokio worker thread (work-stealing) | M:N over a few OS threads |
| reduction counting | Wasmtime epoch interruption | forces fair yields, even in tight loops |
| mailbox | per-process async channel | host copies message bytes across memories |
| `send/2` | `rusm::message::send(pid)` | fire-and-forget to a mailbox |
| `receive` | `rusm::message::receive()` | suspends the process until a message arrives |
| link | bidirectional failure propagation | a crash signals linked peers |
| monitor | one-way failure notification | observe without dying together |
| supervisor | a process that restarts crashing children | "let it crash" |
| `:global` | distributed registry | cluster-wide name → pid |
| `Node.connect/1`, epmd | QUIC + TLS node transport | secure node-to-node links |
| `iex --remsh` | `rusm attach <node>` | live REPL into a running node |
| `:observer` | the dashboard's observer view | live processes, schedulers, memory |
| BEAM | the RUSM runtime (Rust + Tokio + Wasmtime) | the host that runs everything |

Terms specific to Phase 0:

| Term | Meaning |
| --- | --- |
| frame | one sampled tick (throughput, latency, observer snapshot) sent to clients |
| scenario | a named benchmark (e.g. `connection-storm`); synthetic in Phase 0 |
| synthetic source | deterministic generator producing scenario-shaped data per tick |
| detail toggle | switch for the costly per-instance observer table |
