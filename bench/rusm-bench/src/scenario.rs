use serde::{Deserialize, Serialize};

/// The roadmap phase RUSM has reached. A scenario runs on **real** runtime data
/// once its `real_after_phase` is at or below this — bump it as each phase lands.
pub const CURRENT_PHASE: u8 = 11;

/// A benchmark scenario the dashboard can run.
///
/// `real_after_phase` records the roadmap phase at which a scenario switches from
/// synthetic data to measuring the real runtime; see [`CURRENT_PHASE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    SpawnStorm,
    PingPong,
    Fairness,
    FaultRecovery,
    ConnectionStorm,
    ComponentStorm,
    DistributedFanout,
    ModuleStorm,
    StreamPipe,
    ConnectionScale,
    // Serving scenarios (live co-resident demos): a real in-process WASM server
    // driven through the shared `rusm-loadtest` path — balter for HTTP, the
    // connection-capacity harness for WS/SSE. The *fair* headline numbers come from
    // the out-of-process `rusm-loadtest` binary vs a `rusm serve` port.
    HttpThroughput,
    WsEcho,
    SseFanout,
    // TypeScript twins — same servers, served from a Bun-built TS bundle on the
    // rquickjs runners (the RS↔TS comparison).
    HttpThroughputTs,
    WsEchoTs,
    SseFanoutTs,
    // Platform primitives (all real, Phase 11): the durable KV store, pub/sub
    // fan-out, and Web Crypto from a TS guest — the capabilities a real app leans on.
    KvStorm,
    PubSubFanout,
    CryptoOps,
    // Extension primitives (real, 0.4.0): an app's own native **custom bridge** called
    // from a guest, and **dynamic WASM** — a runtime-chosen compiled component, compiled
    // once then served hot from the content-addressed cache.
    CustomBridge,
    DynamicWasm,
}

/// Which guest language a serving scenario runs — the same server hosts a Rust
/// `wasi:http`/actor component or a TypeScript bundle on the rquickjs runners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guest {
    Rust,
    Ts,
}

/// What a scenario's headline throughput number *counts*, so the dashboard can
/// format it correctly (a plain count uses k/M/B; a byte rate uses KB/MB/GB/s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricUnit {
    /// A per-second count — spawns, messages, restarts, …
    Count,
    /// A per-second byte rate — streaming throughput.
    Bytes,
}

/// Display metadata for a [`Scenario`], sent to the dashboard menu and the
/// per-scenario explanation panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioMeta {
    pub id: String,
    pub label: String,
    /// One-line summary (shown in the menu).
    pub description: String,
    /// Longer explanation bullets: what it stresses, the headline metric, what to
    /// watch, and the background — shown above the metrics.
    pub details: Vec<String>,
    pub real_after_phase: u8,
    /// Whether this scenario is driven by the **real** runtime now (vs synthetic),
    /// i.e. `real_after_phase <= CURRENT_PHASE`.
    pub real: bool,
    /// How to read/format the throughput headline (count vs byte rate).
    pub unit: MetricUnit,
    /// Exactly what the throughput counts (e.g. "process spawns/sec", "messages/sec
    /// (ping + pong …)") — so a headline number is unambiguous on the dashboard.
    pub ops_label: String,
    /// What the latency samples measure (e.g. "round-trip", "spawn time"), or `None`
    /// for a throughput-only scenario with no meaningful per-op latency.
    pub latency_label: Option<String>,
    /// The engine's implementation source (Rust), so the dashboard can show how the
    /// scenario is built. `None` for synthetic scenarios. Single source of truth:
    /// this is the actual compiled file via `include_str!`.
    pub source: Option<String>,
    /// The engine source's filename (e.g. `streampipe.rs`), shown as the code
    /// panel's header. `None` for synthetic scenarios.
    pub source_file: Option<String>,
}

impl Scenario {
    // Ordered by the phase each scenario goes live (the dashboard menu shows them
    // in this order). The enum discriminants are unchanged, so the synthetic
    // source stays deterministic.
    // Serving scenarios (HTTP/WS/SSE, + TS twins) are **live co-resident demos** of
    // the serving path; the *fair* throughput headline is measured out-of-process by
    // the `rusm-loadtest` binary against a real `rusm serve` port.
    pub const ALL: [Scenario; 21] = [
        Scenario::SpawnStorm,        // phase 1
        Scenario::PingPong,          // phase 2
        Scenario::FaultRecovery,     // phase 3
        Scenario::ConnectionStorm,   // phase 5
        Scenario::ConnectionScale,   // phase 5 — held-open concurrency to the OS ceiling
        Scenario::Fairness,          // phase 6
        Scenario::ModuleStorm,       // phase 6 — wasip1 core modules (Lunatic head-to-head)
        Scenario::ComponentStorm,    // phase 7
        Scenario::StreamPipe,        // phase 7 — cross-process byte-stream throughput
        Scenario::DistributedFanout, // phase 9
        Scenario::HttpThroughput,    // phase 11 — serve HTTP, steady closed-loop load (Rust)
        Scenario::HttpThroughputTs,  // phase 11 — …the TypeScript twin
        Scenario::WsEcho,            // phase 11 — WebSocket echo, component per connection (Rust)
        Scenario::WsEchoTs,          // phase 11 — …the TypeScript twin
        Scenario::SseFanout,         // phase 11 — Server-Sent Events fan-out (Rust)
        Scenario::SseFanoutTs,       // phase 11 — …the TypeScript twin
        Scenario::KvStorm,           // phase 11 — durable KV write throughput (redb)
        Scenario::PubSubFanout,      // phase 11 — pub/sub 1→N broadcast
        Scenario::CryptoOps,         // phase 11 — crypto.subtle from a TS guest
        Scenario::CustomBridge,      // phase 11 — guest → native custom-bridge call
        Scenario::DynamicWasm,       // phase 11 — runtime-loaded compiled component (cold→hot)
    ];

    /// The guest language a serving scenario runs (Rust by default; the `*Ts`
    /// variants run a TypeScript bundle on the rquickjs runners).
    pub fn guest(self) -> Guest {
        match self {
            Scenario::HttpThroughputTs | Scenario::WsEchoTs | Scenario::SseFanoutTs => Guest::Ts,
            _ => Guest::Rust,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Scenario::SpawnStorm => "spawn-storm",
            Scenario::PingPong => "ping-pong",
            Scenario::Fairness => "fairness",
            Scenario::FaultRecovery => "fault-recovery",
            Scenario::ConnectionStorm => "connection-storm",
            Scenario::ComponentStorm => "component-storm",
            Scenario::DistributedFanout => "distributed-fanout",
            Scenario::ModuleStorm => "module-storm",
            Scenario::StreamPipe => "stream-pipe",
            Scenario::ConnectionScale => "connection-scale",
            Scenario::HttpThroughput => "http-throughput",
            Scenario::WsEcho => "ws-echo",
            Scenario::SseFanout => "sse-fanout",
            Scenario::HttpThroughputTs => "http-throughput-ts",
            Scenario::WsEchoTs => "ws-echo-ts",
            Scenario::SseFanoutTs => "sse-fanout-ts",
            Scenario::KvStorm => "kv-storm",
            Scenario::PubSubFanout => "pubsub-fanout",
            Scenario::CryptoOps => "crypto-ops",
            Scenario::CustomBridge => "custom-bridge",
            Scenario::DynamicWasm => "dynamic-wasm",
        }
    }

    pub fn from_id(id: &str) -> Option<Scenario> {
        Scenario::ALL.into_iter().find(|s| s.id() == id)
    }

    /// What this scenario's throughput number measures — bytes/sec for the byte
    /// pipe, a plain per-second count for everything else.
    pub fn metric_unit(self) -> MetricUnit {
        match self {
            Scenario::StreamPipe => MetricUnit::Bytes,
            _ => MetricUnit::Count,
        }
    }

    /// **Exactly** what the throughput headline counts — so a number can never be
    /// misread (e.g. ping-pong counts *each message*, not round-trips). The dashboard
    /// shows this beside the rate.
    pub fn ops_label(self) -> &'static str {
        match self {
            Scenario::SpawnStorm => "process spawns/sec",
            Scenario::PingPong => "messages/sec (ping + pong — one round-trip is two)",
            Scenario::Fairness => "bystander progress ops/sec (under full-core spinners)",
            Scenario::FaultRecovery => "supervised restarts/sec",
            Scenario::ConnectionStorm => "TCP connections/sec",
            Scenario::ComponentStorm => "component spawns/sec",
            Scenario::DistributedFanout => "cross-node round-trips/sec",
            Scenario::ModuleStorm => "core-module spawns/sec",
            Scenario::StreamPipe => "bytes/sec (consumed chunks × 4 KiB)",
            Scenario::ConnectionScale => {
                "connection reconnects/sec (at the held-connection ceiling)"
            }
            Scenario::HttpThroughput | Scenario::HttpThroughputTs => "HTTP requests/sec (achieved)",
            Scenario::WsEcho | Scenario::WsEchoTs => "WebSocket echo round-trips/sec",
            Scenario::SseFanout | Scenario::SseFanoutTs => "SSE events/sec (across held streams)",
            Scenario::KvStorm => "durable KV read-modify-writes/sec (a redb commit each)",
            Scenario::PubSubFanout => "subscriber deliveries/sec (1 publish → N subscribers)",
            Scenario::CryptoOps => "crypto.subtle SHA-256 digests/sec (TS guest round-trip)",
            Scenario::CustomBridge => "native custom-bridge calls/sec (guest → host round-trip)",
            Scenario::DynamicWasm => "dynamic component spawns/sec (compiled once, then cached)",
        }
    }

    /// What the sampled latency is end-to-end (the dashboard's latency-axis label), or
    /// `None` for a throughput-only scenario where no per-op latency is meaningful.
    pub fn latency_label(self) -> Option<&'static str> {
        match self {
            Scenario::SpawnStorm => Some("spawn time"),
            Scenario::PingPong => Some("round-trip"),
            Scenario::Fairness => None, // a progress counter, not a per-op latency
            Scenario::FaultRecovery => Some("restart latency"),
            Scenario::ConnectionStorm => Some("client-side connect time"),
            Scenario::ComponentStorm => Some("component spawn time"),
            Scenario::DistributedFanout => Some("cross-node round-trip"),
            Scenario::ModuleStorm => Some("core-module spawn time"),
            Scenario::StreamPipe => None, // byte throughput; no per-byte latency
            Scenario::ConnectionScale => Some("connect time"),
            Scenario::HttpThroughput | Scenario::HttpThroughputTs => Some("request latency"),
            Scenario::WsEcho | Scenario::WsEchoTs => Some("echo round-trip"),
            Scenario::SseFanout | Scenario::SseFanoutTs => Some("event delivery latency"),
            Scenario::KvStorm => Some("read-modify-write"),
            Scenario::PubSubFanout => Some("publish → delivery (one-way)"),
            Scenario::CryptoOps => Some("digest round-trip"),
            Scenario::CustomBridge => Some("bridge call round-trip"),
            Scenario::DynamicWasm => Some("dynamic spawn (the first sample is the cold compile)"),
        }
    }

    /// The **engine source file** backing this scenario, embedded at compile time
    /// (`include_str!`) so the dashboard can show the *real* code that produced the
    /// numbers — single source of truth, never a hand-copied snippet. `None` for
    /// scenarios still on synthetic data (no engine yet).
    fn engine_source(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Scenario::SpawnStorm => ("spawnstorm.rs", include_str!("spawnstorm.rs")),
            Scenario::PingPong => ("pingpong.rs", include_str!("pingpong.rs")),
            Scenario::FaultRecovery => ("faultrecovery.rs", include_str!("faultrecovery.rs")),
            Scenario::ConnectionStorm => ("connectionstorm.rs", include_str!("connectionstorm.rs")),
            Scenario::Fairness => ("fairness.rs", include_str!("fairness.rs")),
            Scenario::ModuleStorm => ("modulestorm.rs", include_str!("modulestorm.rs")),
            Scenario::ComponentStorm => ("componentstorm.rs", include_str!("componentstorm.rs")),
            Scenario::StreamPipe => ("streampipe.rs", include_str!("streampipe.rs")),
            Scenario::DistributedFanout => {
                ("distributedfanout.rs", include_str!("distributedfanout.rs"))
            }
            Scenario::ConnectionScale => ("connectionscale.rs", include_str!("connectionscale.rs")),
            // Serving scenarios show the **guest handler you actually write** (the
            // star of the show), not the benchmark harness — Rust or TypeScript.
            Scenario::HttpThroughput => (
                "http-lean/src/lib.rs",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/http-lean/src/lib.rs"),
            ),
            Scenario::HttpThroughputTs => (
                "ts-http-hello/index.ts",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/ts-http-hello/index.ts"),
            ),
            Scenario::WsEcho => (
                "rs-ws-echo/src/lib.rs",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/rs-ws-echo/src/lib.rs"),
            ),
            Scenario::WsEchoTs => (
                "ts-ws-echo/index.ts",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/ts-ws-echo/index.ts"),
            ),
            Scenario::SseFanout => (
                "sse-firehose/src/main.rs",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/sse-firehose/src/main.rs"),
            ),
            Scenario::SseFanoutTs => (
                "ts-sse-firehose/index.ts",
                include_str!("../../../crates/rusm-wasm/tests/fixtures/ts-sse-firehose/index.ts"),
            ),
            // Platform-primitive scenarios show their engine — the real driving code.
            Scenario::KvStorm => ("kvstorm.rs", include_str!("kvstorm.rs")),
            Scenario::PubSubFanout => ("pubsubfanout.rs", include_str!("pubsubfanout.rs")),
            Scenario::CryptoOps => ("cryptoops.rs", include_str!("cryptoops.rs")),
            Scenario::CustomBridge => ("custombridge.rs", include_str!("custombridge.rs")),
            Scenario::DynamicWasm => ("dynamicwasm.rs", include_str!("dynamicwasm.rs")),
        })
    }

    /// The engine's implementation source (the file above its `#[cfg(test)]`
    /// module), trimmed — the "essential benchmark code" for the dashboard panel.
    fn engine_impl_source(self) -> Option<String> {
        self.engine_source().map(|(_, src)| {
            src.split("\n#[cfg(test)]")
                .next()
                .unwrap_or(src)
                .trim_end()
                .to_string()
        })
    }

    pub fn meta(self) -> ScenarioMeta {
        let (label, description, details, real_after_phase) = match self {
            Scenario::SpawnStorm => (
                "Spawn storm",
                "How fast can RUSM CREATE processes? Raw spawn throughput (create + reap).",
                vec![
                    "What's unique here: this stresses *creating* processes — nothing else. (Ping-pong is about messaging, fairness about CPU sharing, connection-storm about how many you can hold open.)",
                    "Headline: spawns/sec — measured ~2.4M/sec. A RUSM process is one Tokio task (plus, for Wasm, one isolated instance), so a spawn is just instantiate + schedule.",
                    "These are NOT OS processes/threads: hundreds of thousands run cooperatively over a few OS threads (the schedulers, ~one per CPU core). The BEAM runs millions; so can we.",
                    "Why it matters: if spawning is this cheap, you can model every request / connection / job as its own crash-isolated process.",
                    "Phase 1: REAL native rusm-otp processes — spawns/sec and spawn latency measured live.",
                ],
                1,
            ),
            Scenario::PingPong => (
                "Message ping-pong",
                "How fast can two processes TALK? Message throughput + round-trip latency.",
                vec![
                    "What's unique here: this stresses *messaging* between existing processes — the mailbox + scheduler hot path — not creating them (spawn storm).",
                    "Headline: messages/sec — measured ~21M/sec, round-trip p50 well under 1µs.",
                    "Each ping carries the sender's pid so the ponger knows whom to reply to; messages move by value, nothing is shared, exactly like Erlang.",
                    "Low, stable round-trip latency under load means wakeups are cheap and scheduling is fair.",
                    "Phase 2: REAL rusm-otp processes exchanging real messages, measured live.",
                ],
                2,
            ),
            Scenario::Fairness => (
                "Fairness under tight loop",
                "Can one CPU-HOGGING process starve the rest? It must not — preemption keeps others running.",
                vec![
                    "\"Fairness\" = fair CPU sharing: no single process may hog a core and starve the others. What's unique here: it's about *CPU sharing*, not throughput. Tight-loop Wasm guests (spinners) try to hog every core while bystander guests must keep working.",
                    "Headline: bystander progress (work/sec) — they keep running at tens of millions of ops/sec (~50M+ under everyday load, past 400M when cores are free) *despite* the spinners. A nonzero rate IS the proof; the absolute number scales with available CPU.",
                    "Phase 6: REAL Wasm. Tokio scheduling is cooperative, so RUSM arms Wasmtime epoch interruption — even an infinite-loop guest yields (the BEAM's reduction-counting idea; lighter than Lunatic's per-instruction fuel — a periodic atomic, ~zero steady-state cost).",
                    "Without preemption, spinners filling every scheduler thread would pin bystanders to zero. They don't.",
                ],
                6,
            ),
            Scenario::FaultRecovery => (
                "Fault recovery",
                "When a process CRASHES, how fast does its supervisor RESTART it?",
                vec![
                    "What's unique here: it's about *crash recovery* — deliberately crashing supervised children and timing how fast they come back. \"Let it crash\", in action.",
                    "Headline: restarts/sec — measured ~285k/sec.",
                    "Each supervisor traps exits and links its children; a crash arrives as an exit signal and the supervisor starts a clean replacement while the rest of the system never notices.",
                    "Why it matters: per-process isolation is what makes crashes survivable rather than fatal.",
                    "Phase 3: REAL rusm-otp supervisors restarting real crashing children, measured live.",
                ],
                3,
            ),
            Scenario::ConnectionStorm => (
                "Connection storm",
                "How MANY live network connections can RUSM hold at once — one process per connection?",
                vec![
                    "What's unique here: it's about *concurrency* — how many simultaneous TCP connections you can keep open, each served by its own isolated process — not raw create speed.",
                    "Headline: peak concurrent connections (the live process count) — thousands at once — plus connections/sec and connect latency.",
                    "Phase 5: REAL loopback TCP, each connection a cheap isolated process, measured live.",
                    "The ceiling is the OS (file descriptors, ephemeral ports, the kernel connect/accept path), NOT RUSM: minting a process per connection is near-free (spawn storm does ~2.4M/sec, hundreds of times this). This tile shows the live loopback rate; the *earned* out-of-process figure comes from `rusm-loadtest conn` against a `rusm serve` port — ~34k establishments/sec even on the heavier path where every connection spawns a full sandboxed component (a raw-TCP process-per-connection, like this tile, is lighter still). RUSM scales with the kernel, not the runtime.",
                ],
                5,
            ),
            Scenario::ComponentStorm => (
                "Component storm",
                "How fast can RUSM LOAD + RUN real WASM components (vs bare processes)?",
                vec![
                    "What's unique here: like the spawn storm, but each process is a real WASI **component** (a sandboxed Wasm instance), so it measures the true cost of *hosting components* — the thing wasmCloud does heavily.",
                    "Headline: component spawns/sec — measured ~440k/sec (instantiate from the pooling allocator + copy-on-write image, then schedule + reap).",
                    "Phase 7: REAL components. The optimized lever set runs live — pooling allocator, CoW, per-module InstancePre, precomputed export index, single runtime-handle clone, zero-overhead default mailbox, park-based backpressure.",
                    "Lunatic hosts only core modules with its own ABI — it has no component-model host at all; matching bare-process spawn economics while hosting real components is the bar we clear here.",
                ],
                7,
            ),
            Scenario::ModuleStorm => (
                "Module storm",
                "How fast can RUSM spawn raw wasip1 core modules — Lunatic's exact domain?",
                vec![
                    "What's unique here: like the spawn storm and component storm, but each process is a raw **wasip1 core module** — the *exact artifact Lunatic hosts*, so it's the apples-to-apples number for a head-to-head.",
                    "Headline: core-module spawns/sec — measured ~475k/sec (instantiate from the pooling allocator + copy-on-write image + precomputed export index, then schedule + reap). Notably this is ~the same as a wasip2 component (~440k): the component model costs almost nothing over a raw core module on RUSM's pooled path. The real gap is to a bare task (~2.4M) — that ~5x is the price of real Wasm memory isolation.",
                    "Phase 6: REAL wasip1. The same lever set as the component path runs live — pooling allocator, CoW, per-module InstancePre, precomputed ModuleExport index, single runtime-handle clone, park-based backpressure.",
                    "The direct Lunatic head-to-head: Lunatic spawns wasip1 core-module processes via on-demand allocation + per-instruction fuel; RUSM recycles pooled instances and preempts with epochs — and hosts components too.",
                ],
                6,
            ),
            Scenario::StreamPipe => (
                "Stream pipe",
                "How fast can one process PIPE a byte stream to another (with back-pressure)?",
                vec![
                    "What's unique here: every other storm measures *creating* processes or *sending one message*; this measures *moving a continuous byte stream* between two processes — one opens a stream to another and floods it with data as fast as the reader will take it.",
                    "Headline: streaming throughput in bytes/sec (so 800,000,000 means ~800 MB/s; this scenario reaches multiple GB/s aggregate across pairs). A producer process streams 4 KiB chunks to a consumer through the runtime; the rate is how many bytes per second make it end to end.",
                    "Back-pressure (the important bit): the stream is a *bounded* channel, so a slow reader automatically slows the writer — the writer's fiber simply parks until there's room, with no unbounded memory growth and no busy-polling. That safety is exactly what lets a component serve HTTP/SSE/WS bodies without falling over.",
                    "Phase 7: REAL streams. Built on the Wasm-free StreamHandle (a Tokio bounded channel) bridged to WASM guests via the actor ABI (stream_open / stream_write / stream_read) — the same byte-stream primitive a native process gets, handed to a sandboxed Wasm process.",
                ],
                7,
            ),
            Scenario::DistributedFanout => (
                "Distributed fan-out",
                "How fast do processes on DIFFERENT machines talk? Cross-node messaging.",
                vec![
                    "What's unique here: all the others run on one node; this one crosses the QUIC + TLS link between separate RUSM nodes (here, several real nodes in-process, each on its own loopback endpoint — a faithful stand-in for separate machines).",
                    "Real (Phase 9): a hub node and a pool of senders each keep one round-trip in flight to worker nodes' echo processes, so the headline is genuine cross-node throughput, not a backlog.",
                    "Headline: cross-node round-trips/sec and round-trip latency over the rusm-cluster transport (~550k cross-node msgs/sec, ~39µs p50 unloaded on loopback).",
                    "Processes message across nodes transparently — distributed Erlang, for WebAssembly — with remote spawn and live attach on the same transport.",
                    "Why it matters: a single machine has limits; horizontal scale needs cheap, secure node-to-node messaging.",
                    "Background: nodes connect like Node.connect/1, and a gossiped global registry resolves process names cluster-wide.",
                ],
                9,
            ),
            Scenario::ConnectionScale => (
                "Connection scale",
                "How MANY connections can RUSM hold at once — pushed to the OS ceiling?",
                vec![
                    "What's unique here: like connection-storm, but it *holds every connection open* instead of recycling — so it measures pure concurrency: how many live connection processes coexist, ramped to the machine's wall.",
                    "Headline: peak concurrent connections (the live process count) — tens of thousands held, each its own isolated rusm-otp process, plus the reconnect rate at the edge.",
                    "The ceiling is the OS, never RUSM: a loopback connection costs 2 file descriptors, so the per-process fd cap is the wall (~61k here). The client sheds the ephemeral-port limit with the 4-tuple trick (disjoint source-port stripes + SO_REUSEADDR across many destination ports), so fds are the only wall left.",
                    "Phase 5: REAL loopback TCP, one process per held connection. A real deployment takes connections from many client hosts, so the per-node ceiling is fds and the fleet ceiling is the Phase 9 cluster — RUSM rides the kernel, with a full supervised process behind every socket.",
                ],
                5,
            ),
            Scenario::HttpThroughput => (
                "HTTP throughput",
                "A sandboxed WASM component serving HTTP under a steady closed-loop load. Requests/sec.",
                vec![
                    "What's unique here: the others measure the actor model; this serves real HTTP — a `wasi:http` component hosted by hyper + wasmtime-wasi-http, one fresh sandboxed instance per request. The live load is **closed-loop** — a fixed set of outstanding requests sized to the server — so it self-limits to the real capacity and holds rock-steady, never flooding or collapsing. (The out-of-process `rusm-loadtest` headline instead uses balter's rate sweep to find the max sustained rate.)",
                    "Headline: achieved requests/sec and per-request latency, charted live. The response is produced BY THE GUEST; the host only moves bytes. A trap fails just that request.",
                    "This tile is a **co-resident live demo** (load + server share this node process). The *fair* headline number is measured **out-of-process** by `rusm-loadtest` against a real `rusm serve` port — ~46k req/s at 0% errors on loopback — so the generator never steals the server's CPU.",
                    "Why it matters: run your low-code/LLM/web component as a sandboxed, supervised HTTP server — `rusm serve` hosts it on a real port.",
                ],
                11,
            ),
            Scenario::WsEcho => (
                "WebSocket echo",
                "One sandboxed component PROCESS per WebSocket connection. Echo round-trips/sec.",
                vec![
                    "What's unique here: every connection is served by its own WASM **component process** (`WasmRuntime::ws_server`), not a shared event loop — inbound frame → the process mailbox, reply via a Wasm-free writer process that owns the socket sink. Clean actor isolation per socket.",
                    "Headline: echo round-trips/sec and round-trip latency across many held connections, charted live. The load holds real WS connections through the shared connection-capacity harness (the same `rusm-loadtest` path).",
                    "A **co-resident live demo**; the fair out-of-process figure (`rusm-loadtest`) is ~146k round-trips/s across 256 held connections on loopback. A handler crash drops only that socket — never the listener or the other connections.",
                    "Why it matters: per-connection isolation + supervision is what a single shared event loop (the usual WS server) can't give you.",
                ],
                11,
            ),
            Scenario::SseFanout => (
                "SSE fan-out",
                "Many live Server-Sent Event streams, each its own WASM instance. Events/sec.",
                vec![
                    "What's unique here: many long-lived `text/event-stream` connections, each served by its own `wasi:http` component instance streaming events as fast as the client drains them — the 'many concurrent streaming responses, all held' story.",
                    "Headline: events/sec across all streams + live concurrency, charted live. Streams are held open by the shared connection-capacity harness (the `rusm-loadtest` path).",
                    "A **co-resident live demo**; the fair out-of-process figure (`rusm-loadtest`) is ~609k events/s across 256 held streams on loopback. A dropped client tears down only its own instance.",
                    "Why it matters: streaming fan-out to many subscribers is the shape of live dashboards, LLM token streams, and push feeds.",
                ],
                11,
            ),
            Scenario::HttpThroughputTs => (
                "HTTP throughput (TS)",
                "Same HTTP serving under a steady closed-loop load — but the handler is a TypeScript component.",
                vec![
                    "What's unique here: identical to HTTP throughput, but the response is produced by a TYPESCRIPT HTTP handler on the embedded rquickjs js-http-runner (`export default` a request→response function — server-side), not a Rust `wasi:http` component — the RS↔TS comparison.",
                    "Headline: achieved requests/sec + latency, charted live. The closed-loop load holds a fixed set of outstanding requests, so it sits steady at the TS path's real ceiling (no flood, no collapse). Each request instantiates a fresh sandboxed JS instance that runs the handler.",
                    "Expect LOWER throughput than the Rust path — the honest cost of evaluating JS per request (rquickjs). A **co-resident live demo**; the fair figure comes from `rusm-loadtest`.",
                    "Why it matters: write your handler in TS and RUSM still serves it sandboxed and supervised.",
                ],
                11,
            ),
            Scenario::WsEchoTs => (
                "WebSocket echo (TS)",
                "Same WS serving — but each connection is a TypeScript worker process.",
                vec![
                    "What's unique here: identical to WebSocket echo, but each connection's handler is a TYPESCRIPT worker on the js-runner — inbound frame → mailbox → echo, one sandboxed JS process per socket.",
                    "Headline: echo round-trips/sec + latency across held connections, charted live (the shared capacity harness holds the connections).",
                    "The JS runner adds per-message overhead vs the Rust component, so throughput is lower — the honest TS cost; the concurrency story is the same. A **co-resident live demo**; the fair figure comes from `rusm-loadtest`.",
                    "Why it matters: per-connection isolation + supervision, with the handler written in TypeScript.",
                ],
                11,
            ),
            Scenario::SseFanoutTs => (
                "SSE fan-out (TS)",
                "Same SSE serving — but the event stream is a TypeScript ReadableStream.",
                vec![
                    "What's unique here: identical to SSE fan-out, but the events come from a TYPESCRIPT handler returning a Response whose body is a ReadableStream, on the js-http-runner — pulled chunk-by-chunk and flushed incrementally (true streaming).",
                    "Headline: events/sec across many held streams, charted live (the shared capacity harness holds the streams).",
                    "The rquickjs pull per event costs more than the Rust path, so events/sec is lower — the honest TS cost; the held-stream concurrency is the same. A **co-resident live demo**; the fair figure comes from `rusm-loadtest`.",
                    "Why it matters: push streaming written in TypeScript, sandboxed and supervised per stream.",
                ],
                11,
            ),
            Scenario::KvStorm => (
                "KV storm",
                "Durable, ACID key-value writes under load — how fast can processes COMMIT?",
                vec![
                    "What's unique here: this is the only scenario that touches DISK. Worker processes hammer a shared embedded KV store (rusm-kv, over redb) with read-modify-writes — the durable-counter / session-update workload — and EVERY write is its own ACID commit.",
                    "Headline: durable read-modify-writes/sec (a get + an fsync'd commit each), plus the end-to-end RMW latency. This is the honest *durable* write rate, not an in-memory one.",
                    "redb serialises writers behind one commit lock while readers run concurrently (MVCC), so adding workers past the core count mostly deepens the commit queue — the number is the durable-write ceiling, and it scales with the disk, not the runtime.",
                    "Phase 11: REAL rusm-kv — the same embedded store a guest reaches through the storage capability (no Redis, no external daemon). A focused durable primitive, Wasm-free, like rusm-otp is for processes.",
                ],
                11,
            ),
            Scenario::PubSubFanout => (
                "Pub/sub fan-out",
                "One publish, MANY subscribers — live 1→N broadcast throughput.",
                vec![
                    "What's unique here: it's about *fan-out* — a publisher process broadcasting each message to a whole set of subscriber processes at once, not a 1:1 round-trip (ping-pong) or process creation (spawn storm).",
                    "Headline: subscriber deliveries/sec — one publish counts as N deliveries — plus the one-way publish→delivery latency measured at a real subscriber.",
                    "This is exactly the mechanics of rusm-rs's `pubsub::Topics::publish` (`for sub in subscribers { send(sub, msg) }`), the broker primitive a guest embeds — with crash-safe pruning via monitors. The publisher is the bottleneck; subscribers just drain, so nothing grows unbounded.",
                    "Phase 11: REAL rusm-otp processes — a publisher fanning real messages out to live subscriber processes, measured end to end.",
                ],
                11,
            ),
            Scenario::CryptoOps => (
                "Crypto ops (TS)",
                "Web Crypto from a sandboxed TypeScript guest — SHA-256 digests/sec.",
                vec![
                    "What's unique here: it measures real cryptography (`crypto.subtle.digest`) served by a TYPESCRIPT guest on the embedded rquickjs runner — native RustCrypto behind the Web Crypto ABI, in a sandboxed process that needs no capability.",
                    "Headline: SHA-256 digests/sec + per-digest round-trip latency. Each request hashes a fixed payload and replies; the rate is the honest cost of *offering* crypto from a JS guest (the rquickjs call + the message round-trip).",
                    "Why it matters: the entropy + crypto ecosystem (uuid, jwt, hashing, AES-GCM) runs unchanged inside a RUSM TS component — sandboxed, supervised, and backed by audited Rust crypto, not a JS reimplementation.",
                    "Phase 11: REAL rquickjs guests driving real crypto.subtle calls, measured live.",
                ],
                11,
            ),
            Scenario::CustomBridge => (
                "Custom bridge",
                "How fast can a guest call an app's own NATIVE host function (a custom bridge)?",
                vec![
                    "What's unique here: each request is a typed call from a sandboxed guest into a native host function the app registered — a `demo:bridge/greet` custom bridge, wired via `WasmRuntime::with_bridges`, exactly the `bridges/<name>/host.rs` seam a real app uses. No dispatcher, no broker, no RPC — a plain WIT import resolved to Rust.",
                    "Headline: native bridge calls/sec + per-call round-trip latency. Each guest calls `greet(\"World\")` in a loop; the host composes the reply (stamped with the calling pid) and returns it typed. The rate is the honest cost of a custom bridge call (the typed WIT call + the message round-trip to the driver).",
                    "Why it matters: this is RUSM's compiled-in answer to a wasmCloud capability provider — default-deny, gated by name, identical for Rust/Go/TS guests — at in-process speed, no lattice.",
                    "Phase 11 (0.4.0): REAL guests calling a real app-registered native bridge, measured live.",
                ],
                11,
            ),
            Scenario::DynamicWasm => (
                "Dynamic WASM",
                "How fast can RUSM run a COMPILED component chosen at runtime — cold once, then hot?",
                vec![
                    "What's unique here: unlike the component storm (a pre-prepared handle), each spawn goes through the content-addressed compile cache — `prepare_dynamic(source)` then spawn — exactly the path a guest's `spawn-from \"kv:…\"` takes. The runtime *chooses the component at run time*.",
                    "Headline: dynamic component spawns/sec + per-spawn latency. The bundle is compiled ONCE on the first spawn (cold — the first latency sample shows the one-time compile cost, ~17 ms for a small component), then every later spawn is a cache hit instantiating on the pooled fast path (hot — ~0.5 ms). The rate is the steady-state HOT throughput.",
                    "Why it matters: it proves runtime-loaded, operator-sandboxed plugins spawn at essentially built-in speed — the compile is paid once and cached (keyed by the SHA-256 of the bytes), single-flighted, with a freshness TTL. Untrusted/per-tenant compiled code, hot.",
                    "Phase 11 (0.4.0): REAL components compiled + cached + spawned through the dynamic path, measured live.",
                ],
                11,
            ),
        };
        ScenarioMeta {
            id: self.id().to_string(),
            label: label.to_string(),
            description: description.to_string(),
            details: details.into_iter().map(str::to_string).collect(),
            real_after_phase,
            real: real_after_phase <= CURRENT_PHASE,
            unit: self.metric_unit(),
            ops_label: self.ops_label().to_string(),
            latency_label: self.latency_label().map(str::to_string),
            source: self.engine_impl_source(),
            source_file: self.engine_source().map(|(name, _)| name.to_string()),
        }
    }

    pub fn all_meta() -> Vec<ScenarioMeta> {
        Scenario::ALL.into_iter().map(Scenario::meta).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        let mut ids = std::collections::HashSet::new();
        for s in Scenario::ALL {
            let id = s.id();
            assert!(ids.insert(id), "duplicate id {id}");
            assert_eq!(Scenario::from_id(id), Some(s));
        }
    }

    #[test]
    fn from_id_rejects_unknown() {
        assert_eq!(Scenario::from_id("nope"), None);
    }

    #[test]
    fn only_the_byte_pipe_reports_a_byte_rate() {
        assert_eq!(Scenario::StreamPipe.metric_unit(), MetricUnit::Bytes);
        for s in Scenario::ALL {
            if s != Scenario::StreamPipe {
                assert_eq!(s.metric_unit(), MetricUnit::Count, "{} is a count", s.id());
            }
        }
        // The unit travels in the meta the dashboard receives.
        assert_eq!(Scenario::StreamPipe.meta().unit, MetricUnit::Bytes);
    }

    #[test]
    fn live_scenarios_carry_their_engine_source() {
        // Single source of truth: the meta ships the real engine file (impl only,
        // no test module); synthetic scenarios ship none.
        let spawn = Scenario::SpawnStorm.meta().source.unwrap();
        assert!(spawn.contains("struct SpawnStormEngine"));
        assert!(
            !spawn.contains("#[cfg(test)]"),
            "tests are stripped from the panel"
        );
        assert!(Scenario::StreamPipe
            .meta()
            .source
            .unwrap()
            .contains("StreamPipeEngine"));
        assert!(Scenario::DistributedFanout
            .meta()
            .source
            .unwrap()
            .contains("DistributedFanoutEngine"));
    }

    #[test]
    fn serde_uses_kebab_case_id() {
        let json = serde_json::to_string(&Scenario::ConnectionStorm).unwrap();
        assert_eq!(json, "\"connection-storm\"");
        let back: Scenario = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Scenario::ConnectionStorm);
    }

    #[test]
    fn meta_matches_id_and_is_populated() {
        for meta in Scenario::all_meta() {
            assert!(Scenario::from_id(&meta.id).is_some());
            assert!(!meta.label.is_empty());
            assert!(!meta.description.is_empty());
            assert!(meta.details.len() >= 3, "{} needs real detail", meta.id);
            assert!(meta.details.iter().all(|d| !d.is_empty()));
            assert!((1..=11).contains(&meta.real_after_phase));
            // Every headline states exactly what it counts (so a number can't be misread).
            assert!(!meta.ops_label.is_empty(), "{} needs an ops_label", meta.id);
            // A latency label, when present, is non-empty.
            assert!(
                meta.latency_label.as_deref() != Some(""),
                "{} latency_label must not be empty",
                meta.id
            );
        }
    }

    #[test]
    fn only_throughput_only_scenarios_omit_a_latency_label() {
        // Latency is `None` exactly for the two scenarios whose engines record no
        // per-op latency (a fairness progress counter; raw byte throughput) — every
        // other scenario labels what its sampled latency means.
        for s in Scenario::ALL {
            let has = s.latency_label().is_some();
            let expects = !matches!(s, Scenario::Fairness | Scenario::StreamPipe);
            assert_eq!(has, expects, "{} latency_label presence is wrong", s.id());
        }
    }

    #[test]
    fn all_meta_covers_every_scenario() {
        assert_eq!(Scenario::all_meta().len(), Scenario::ALL.len());
    }

    #[test]
    fn graduated_scenarios_are_marked_real() {
        let real: Vec<String> = Scenario::all_meta()
            .into_iter()
            .filter(|m| m.real)
            .map(|m| m.id)
            .collect();
        // Every scenario has a real engine (CURRENT_PHASE = 11) — nothing synthetic.
        // The serving scenarios are live co-resident demos (closed-loop HTTP / capacity harness);
        // the fair headline is measured out-of-process by `rusm-loadtest`.
        assert_eq!(
            real,
            vec![
                "spawn-storm",
                "ping-pong",
                "fault-recovery",
                "connection-storm",
                "connection-scale",
                "fairness",
                "module-storm",
                "component-storm",
                "stream-pipe",
                "distributed-fanout",
                "http-throughput",
                "http-throughput-ts",
                "ws-echo",
                "ws-echo-ts",
                "sse-fanout",
                "sse-fanout-ts",
                "kv-storm",
                "pubsub-fanout",
                "crypto-ops",
                "custom-bridge",
                "dynamic-wasm",
            ]
        );
    }

    #[test]
    fn scenarios_are_listed_in_phase_order() {
        let phases: Vec<u8> = Scenario::all_meta()
            .iter()
            .map(|m| m.real_after_phase)
            .collect();
        let mut sorted = phases.clone();
        sorted.sort_unstable();
        assert_eq!(phases, sorted, "the scenario menu must be phase-ordered");
    }

    #[test]
    fn meta_round_trips_through_json() {
        let metas = Scenario::all_meta();
        let json = serde_json::to_string(&metas).unwrap();
        let back: Vec<ScenarioMeta> = serde_json::from_str(&json).unwrap();
        assert_eq!(metas, back);
    }
}
