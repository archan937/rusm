# `connection_scale` — how many connections can RUSM hold at once?

Pushes concurrent **held-open** connections to the machine's ceiling — each served by
its own isolated `rusm-otp` process — and reports the measured peak. Where
the [`connection-storm`](../../../bench/rusm-bench/src/connectionstorm.rs) scenario measures connect/sec *throughput* by
recycling against one port, this **holds every connection open**, so the question is
pure concurrency: how many live processes-with-sockets can coexist.

```sh
cargo run --release -p rusm-bench --example connection_scale -- [target] [client_tasks]
```

With no arguments it targets **80% of the system's fd ceiling** automatically.

Representative output (this machine — 10 cores, 32 GB, macOS):

```
Connection scale: target 49152 held connections (80% of the ~61440 fd ceiling)
fd limit 122880 (2 fds/connection, both ends in-process), 34 ports, 256 client tasks

  held     7272   (+  7272/0.5s)   connect failures: 0
  held    19106   (+ 11834/0.5s)   connect failures: 0
  held    33065   (+ 13959/0.5s)   connect failures: 0
  held    47012   (+ 13947/0.5s)   connect failures: 0
  held    49407   (+  2395/0.5s)   connect failures: 0

── peak ──
concurrent connections held: 49407   (each its own rusm-otp process)
ramp:                        19716 connections/sec over 2.5s
fd ceiling:                  122880 fds ≈ 61440 connections (2 fds each)
```

## What the ceiling actually is (and what it isn't)

The wall is the **OS**, never RUSM. Two real limits, both the kernel's:

1. **File descriptors.** Loopback puts *both* ends in this one process, so a connection
   costs **2 fds**. `rlimit::increase_nofile_limit` raises us to the per-process cap
   (`kern.maxfilesperproc` = 122,880 here), so ~61,440 connections — we target 80% of
   that. Raising `kern.maxfilesperproc` (root) goes higher.
2. **Ephemeral source ports.** A naive loopback client exhausts its ~16k ephemeral
   ports (49152–65535) long before the fd cap. This bench **dodges that** with the
   *4-tuple trick*: each client task owns a disjoint stripe of explicit source ports
   (bound with `SO_REUSEADDR`), each paired with all 34 destination ports — so every
   `(src_port, dst_port)` 4-tuple is unique and no task races another to bind a port
   (hence **zero connect failures**). That lifts the wall back to the fd cap.

**RUSM itself is nowhere near a limit here.** Minting a process per connection is
near-free — the [spawn storm](../../../bench/rusm-bench/src/spawnstorm.rs) does ~2.4M
spawns/sec, and the process side holds *millions*. The 49k number is what *this single
machine's kernel* allows two-ended on loopback; a real deployment gets connections from
many client hosts (each with its own ephemeral range) and across machines in a cluster,
so the per-node ceiling is fds, and the fleet ceiling is the cluster. The takeaway:
**the connection ceiling tracks the OS, and RUSM rides it with a full supervised process
behind every socket.**
