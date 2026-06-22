use rusm_metrics::{LatencyHistogram, TimeSeries};
use rusm_observer::{NodeSample, Observer};

use crate::componentstorm::ComponentStormEngine;
use crate::connectionscale::ConnectionScaleEngine;
use crate::connectionstorm::ConnectionStormEngine;
use crate::cryptoops::CryptoOpsEngine;
use crate::custombridge::CustomBridgeEngine;
use crate::distributedfanout::DistributedFanoutEngine;
use crate::dynamicwasm::DynamicWasmEngine;
use crate::fairness::FairnessEngine;
use crate::faultrecovery::FaultRecoveryEngine;
use crate::kvstorm::KvStormEngine;
use crate::modulestorm::ModuleStormEngine;
use crate::pingpong::PingPongEngine;
use crate::profile::ResourceProfile;
use crate::profile_tuning::ProfileTuning;
use crate::protocol::Frame;
use crate::pubsubfanout::PubSubFanoutEngine;
use crate::sample::Sample;
use crate::scenario::Scenario;
use crate::serving::{CapacityKind, CapacityServingEngine, HttpServingEngine};
use crate::spawnstorm::SpawnStormEngine;
use crate::streampipe::StreamPipeEngine;
use crate::synthetic::SyntheticSource;

fn available_cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

#[derive(Debug, Clone, Copy)]
pub struct RunnerConfig {
    pub scheduler_count: usize,
    pub max_detail: usize,
    pub latency_samples: usize,
    pub throughput_window: usize,
    /// Sampling rate; ties `ops_per_sec` to a per-tick operation count.
    pub ticks_per_second: u32,
    /// Latency **warm-up**: latencies from the first `warmup_ticks` ticks of a run are
    /// discarded, so the reported p50/p95/p99 are *steady-state* — not inflated by
    /// ramp-up (pool fill, JIT/cache warming). `0` records from the first tick (used by
    /// tests for deterministic per-tick assertions); production sets ~2s worth.
    pub warmup_ticks: u64,
    /// Background spawner tasks for the real spawn-storm engine (one per core).
    pub spawn_workers: usize,
    /// Spawn-storm backpressure: max live processes before spawners pause.
    pub spawn_max_in_flight: usize,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        // Spawn knobs come from the default (Balanced) resource profile.
        let (spawn_workers, spawn_max_in_flight) =
            ResourceProfile::default().tuning(available_cores());
        Self {
            scheduler_count: 8,
            max_detail: 64,
            latency_samples: 64,
            throughput_window: 120,
            ticks_per_second: 20,
            // The library default records latencies from tick 0 (deterministic, what
            // tests assert); the production builder (`runner_config`) sets a real
            // warm-up so the dashboard/CLI report steady-state percentiles.
            warmup_ticks: 0,
            spawn_workers,
            spawn_max_in_flight,
        }
    }
}

/// The runner config implied by a node manifest (just the sampling rate; the
/// `profile` is applied to the running node separately, so it shows up in frames
/// and can be changed live).
pub fn runner_config(cfg: &rusm_node::NodeConfig) -> RunnerConfig {
    let ticks_per_second = cfg.node.ticks_per_second.max(1);
    RunnerConfig {
        ticks_per_second,
        // Steady-state latency: discard the first ~2s of samples so reported
        // percentiles aren't inflated by ramp-up. This is the path the dashboard and
        // `rusm-bench run` use, so published numbers are warmed.
        warmup_ticks: u64::from(ticks_per_second).saturating_mul(WARMUP_SECS),
        ..RunnerConfig::default()
    }
}

/// Seconds of latency samples discarded at the start of a run (see
/// [`RunnerConfig::warmup_ticks`]) — long enough to ride out pool fill + cache/JIT
/// warming, short enough that a short run still has a steady-state window.
const WARMUP_SECS: u64 = 2;

/// The data source driving a run: a real engine per scenario, or a deterministic
/// [`SyntheticSource`] for the runtime-free preview mode ([`Runner::start_synthetic`]).
enum Engine {
    Synthetic(SyntheticSource),
    SpawnStorm(SpawnStormEngine),
    PingPong(PingPongEngine),
    FaultRecovery(FaultRecoveryEngine),
    ConnectionStorm(ConnectionStormEngine),
    Fairness(FairnessEngine),
    ModuleStorm(ModuleStormEngine),
    ComponentStorm(ComponentStormEngine),
    StreamPipe(StreamPipeEngine),
    DistributedFanout(DistributedFanoutEngine),
    ConnectionScale(ConnectionScaleEngine),
    // Serving (live co-resident demos): HTTP via balter, WS/SSE via the capacity harness.
    HttpThroughput(HttpServingEngine),
    Capacity(CapacityServingEngine),
    // Platform primitives.
    KvStorm(KvStormEngine),
    PubSubFanout(PubSubFanoutEngine),
    CryptoOps(CryptoOpsEngine),
    // Extension primitives.
    CustomBridge(CustomBridgeEngine),
    DynamicWasm(DynamicWasmEngine),
}

impl Engine {
    /// The real engine backing a scenario. Every scenario now has one; they scale
    /// their worker/pair/node count with the resource profile.
    fn for_scenario(scenario: Scenario, config: &RunnerConfig) -> Self {
        match scenario {
            Scenario::SpawnStorm => Engine::SpawnStorm(SpawnStormEngine::new(
                config.spawn_workers,
                config.scheduler_count,
                config.spawn_max_in_flight,
            )),
            Scenario::PingPong => Engine::PingPong(PingPongEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::FaultRecovery => Engine::FaultRecovery(FaultRecoveryEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::ConnectionStorm => Engine::ConnectionStorm(ConnectionStormEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::Fairness => Engine::Fairness(FairnessEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::ModuleStorm => Engine::ModuleStorm(ModuleStormEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::ComponentStorm => Engine::ComponentStorm(ComponentStormEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::StreamPipe => Engine::StreamPipe(StreamPipeEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::DistributedFanout => Engine::DistributedFanout(DistributedFanoutEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::ConnectionScale => Engine::ConnectionScale(ConnectionScaleEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::HttpThroughput | Scenario::HttpThroughputTs => {
                Engine::HttpThroughput(HttpServingEngine::new(
                    config.spawn_workers,
                    config.scheduler_count,
                    scenario.guest(),
                ))
            }
            Scenario::WsEcho | Scenario::WsEchoTs => Engine::Capacity(CapacityServingEngine::new(
                config.spawn_workers,
                config.scheduler_count,
                CapacityKind::Ws,
                scenario.guest(),
            )),
            Scenario::SseFanout | Scenario::SseFanoutTs => {
                Engine::Capacity(CapacityServingEngine::new(
                    config.spawn_workers,
                    config.scheduler_count,
                    CapacityKind::Sse,
                    scenario.guest(),
                ))
            }
            Scenario::KvStorm => Engine::KvStorm(KvStormEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::PubSubFanout => Engine::PubSubFanout(PubSubFanoutEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::CryptoOps => Engine::CryptoOps(CryptoOpsEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::CustomBridge => Engine::CustomBridge(CustomBridgeEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
            Scenario::DynamicWasm => Engine::DynamicWasm(DynamicWasmEngine::new(
                config.spawn_workers,
                config.scheduler_count,
            )),
        }
    }

    fn tick(&mut self, tick: u64, config: &RunnerConfig) -> Sample {
        match self {
            Engine::Synthetic(source) => source.tick(
                tick,
                config.latency_samples,
                config.max_detail,
                config.scheduler_count,
            ),
            Engine::SpawnStorm(engine) => engine.tick(),
            Engine::PingPong(engine) => engine.tick(),
            Engine::FaultRecovery(engine) => engine.tick(),
            Engine::ConnectionStorm(engine) => engine.tick(),
            Engine::Fairness(engine) => engine.tick(),
            Engine::ModuleStorm(engine) => engine.tick(),
            Engine::ComponentStorm(engine) => engine.tick(),
            Engine::StreamPipe(engine) => engine.tick(),
            Engine::DistributedFanout(engine) => engine.tick(),
            Engine::ConnectionScale(engine) => engine.tick(),
            Engine::HttpThroughput(engine) => engine.tick(),
            Engine::Capacity(engine) => engine.tick(),
            Engine::KvStorm(engine) => engine.tick(),
            Engine::PubSubFanout(engine) => engine.tick(),
            Engine::CryptoOps(engine) => engine.tick(),
            Engine::CustomBridge(engine) => engine.tick(),
            Engine::DynamicWasm(engine) => engine.tick(),
        }
    }
}

struct RunState {
    scenario: Scenario,
    engine: Engine,
    tick: u64,
    peak_concurrent: u64,
    /// Whether throughput has been seen at least once (so warm-up zeros don't trip
    /// the stall warning).
    warmed: bool,
    /// Consecutive ticks reporting 0 ops/sec *after* warm-up — the stall detector.
    stall_ticks: u64,
    /// Whether the current stall episode has already been warned about (warn once).
    stall_warned: bool,
    /// Counts this engine as live for the duration of the `RunState` (test-only) — drops
    /// with it, so a per-runner peak gauge can prove `start()` releases the prior engine
    /// before building the next (never two pool reservations at once). See [`EngineGauge`].
    #[cfg(test)]
    _live: EngineToken,
}

/// Per-runner gauge of concurrently-live engines (test-only). Held by the [`Runner`];
/// each [`RunState`] holds an [`EngineToken`] minted from it, so the count tracks real
/// engine lifetimes (not source intent) and is isolated per runner — immune to the
/// cross-test contamination a global static would suffer under parallel `cargo test`.
#[cfg(test)]
#[derive(Clone, Default)]
struct EngineGauge {
    current: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
struct EngineToken {
    current: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl EngineToken {
    fn new(gauge: &EngineGauge) -> Self {
        use std::sync::atomic::Ordering::SeqCst;
        let live = gauge.current.fetch_add(1, SeqCst) + 1;
        gauge.peak.fetch_max(live, SeqCst);
        Self {
            current: gauge.current.clone(),
        }
    }
}

#[cfg(test)]
impl Drop for EngineToken {
    fn drop(&mut self) {
        self.current
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// After throughput has started, this many consecutive zero-ops ticks while a
/// scenario is "running" is treated as a stall and logged loudly. At the default
/// 20 Hz that's ~2s — long enough to ride out a balter window boundary, short enough
/// to surface a real silent-zero promptly.
const STALL_TICKS: u64 = 40;

/// Drives a benchmark run and aggregates each tick into a [`Frame`].
///
/// The runner is the synchronous heart of the harness: a transport (the
/// WebSocket server, or the terminal runner) owns the clock and calls
/// [`Runner::tick`] on a cadence, broadcasting the returned frame. Keeping it
/// clock-free makes the whole pipeline deterministic and unit-testable.
pub struct Runner {
    config: RunnerConfig,
    profile: ResourceProfile,
    observer: Observer,
    latency: LatencyHistogram,
    throughput: TimeSeries,
    run: Option<RunState>,
    #[cfg(test)]
    engine_gauge: EngineGauge,
}

impl Runner {
    pub fn new(config: RunnerConfig) -> Self {
        Self {
            observer: fresh_observer(&config, true),
            latency: LatencyHistogram::new(),
            throughput: TimeSeries::new(config.throughput_window),
            run: None,
            profile: ResourceProfile::default(),
            config,
            #[cfg(test)]
            engine_gauge: EngineGauge::default(),
        }
    }

    /// Peak number of engines that were ever live at the same instant (test-only).
    /// `1` proves `start()` drops the previous engine before building the next.
    #[cfg(test)]
    fn peak_live_engines(&self) -> usize {
        self.engine_gauge
            .peak
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn resource_profile(&self) -> ResourceProfile {
        self.profile
    }

    /// Switches the resource profile — re-resolving how many spawn workers and
    /// how large an in-flight cap the storm uses — and re-applies it to a live
    /// spawn-storm run.
    pub fn set_resource_profile(&mut self, profile: ResourceProfile) {
        let (workers, cap) = profile.tuning(available_cores());
        self.config.spawn_workers = workers;
        self.config.spawn_max_in_flight = cap;
        self.profile = profile;
        // Restart whichever real engine is running so the new tuning takes effect.
        if let Some(
            scenario @ (Scenario::SpawnStorm
            | Scenario::PingPong
            | Scenario::FaultRecovery
            | Scenario::ConnectionStorm
            | Scenario::Fairness
            | Scenario::ModuleStorm
            | Scenario::ComponentStorm
            | Scenario::StreamPipe
            | Scenario::DistributedFanout
            | Scenario::ConnectionScale
            | Scenario::HttpThroughput
            | Scenario::WsEcho
            | Scenario::SseFanout
            | Scenario::HttpThroughputTs
            | Scenario::WsEchoTs
            | Scenario::SseFanoutTs
            | Scenario::KvStorm
            | Scenario::PubSubFanout
            | Scenario::CryptoOps),
        ) = self.scenario()
        {
            self.start(scenario);
        }
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    pub fn scenario(&self) -> Option<Scenario> {
        self.run.as_ref().map(|r| r.scenario)
    }

    /// Starts (or restarts) `scenario` on its real engine, resetting all metrics to
    /// a clean slate. Must be called within a Tokio runtime (engines bind sockets /
    /// spawn tasks).
    pub fn start(&mut self, scenario: Scenario) {
        // Tear the previous run's engine down *before* building the next. Each serving
        // engine owns a `WasmRuntime` (a pooling-allocator virtual-memory reservation) plus
        // a server task and hundreds of connection processes; building the new engine while
        // the old is still live makes their two reservations — and the teardown churn —
        // contend, which intermittently starves the new server's connection handshakes past
        // the load harness's connect budget. The new run then never warms: a flat-zero
        // "dead" run until you press Run again. Dropping first frees the old reservation
        // synchronously and signals its aborts up front, so the new engine builds clean.
        self.run = None;
        let engine = Engine::for_scenario(scenario, &self.config);
        self.start_with(scenario, engine);
    }

    /// Starts `scenario` on **deterministic synthetic data** instead of its real
    /// engine — a runtime-free "demo/preview" mode (reproducible per `(scenario,
    /// tick)`), used for dashboard/UI development and as the deterministic fixture
    /// for the runner's own tests.
    pub fn start_synthetic(&mut self, scenario: Scenario) {
        self.start_with(scenario, Engine::Synthetic(SyntheticSource::new(scenario)));
    }

    fn start_with(&mut self, scenario: Scenario, engine: Engine) {
        let detail = self.observer.detail_enabled();
        self.observer = fresh_observer(&self.config, detail);
        self.latency.clear();
        self.throughput.clear();
        self.run = Some(RunState {
            scenario,
            engine,
            tick: 0,
            peak_concurrent: 0,
            warmed: false,
            stall_ticks: 0,
            stall_warned: false,
            #[cfg(test)]
            _live: EngineToken::new(&self.engine_gauge),
        });
    }

    pub fn stop(&mut self) {
        self.run = None;
    }

    pub fn set_observer_detail(&self, enabled: bool) {
        self.observer.set_detail_enabled(enabled);
    }

    pub fn observer_detail_enabled(&self) -> bool {
        self.observer.detail_enabled()
    }

    pub fn tick(&mut self, uptime_ms: u64) -> Frame {
        let Some(state) = self.run.as_mut() else {
            let idle = NodeSample {
                process_count: 0,
                running: 0,
                waiting: 0,
                total_memory_bytes: 0,
                scheduler_load: &[],
                processes: &[],
            };
            return Frame {
                scenario: None,
                running: false,
                uptime_ms,
                ops_per_sec: 0.0,
                peak_concurrent: 0,
                profile: self.profile.id().to_string(),
                latency: self.latency.snapshot(),
                throughput: self.throughput.snapshot(),
                observer: self.observer.snapshot(uptime_ms, idle),
            };
        };

        let t = state.engine.tick(state.tick, &self.config);
        state.tick += 1;
        state.peak_concurrent = state.peak_concurrent.max(t.process_count);
        let scenario = state.scenario;
        let peak_concurrent = state.peak_concurrent;
        // Steady-state latency: ignore samples until the warm-up window has elapsed, so
        // the percentiles reflect the warm system, not pool fill / cache warming.
        let past_warmup = state.tick > self.config.warmup_ticks;

        // Loud-stall invariant: once a scenario has produced throughput, a sustained
        // run of zero ticks means it stalled — surface it instead of quietly charting
        // a 0 (the silent-zero failure mode). Warns once per episode; resets on resume.
        if t.ops_per_sec > 0.0 {
            state.warmed = true;
            state.stall_ticks = 0;
            state.stall_warned = false;
        } else if state.warmed {
            state.stall_ticks += 1;
            if state.stall_ticks == STALL_TICKS && !state.stall_warned {
                eprintln!(
                    "warning: scenario '{}' is running but has reported 0 throughput for {} \
                     consecutive ticks — likely stalled, not a quiet zero",
                    scenario.id(),
                    state.stall_ticks
                );
                state.stall_warned = true;
            }
        }

        if past_warmup {
            for latency_ns in &t.latencies_ns {
                self.latency.record_nanos(*latency_ns);
            }
        }
        self.throughput.push(t.ops_per_sec);

        let ops_this_tick = (t.ops_per_sec / self.config.ticks_per_second as f64) as u64;
        self.observer.record_messages(ops_this_tick);

        let sample = NodeSample {
            process_count: t.process_count as usize,
            running: t.running as usize,
            waiting: t.waiting as usize,
            total_memory_bytes: t.total_memory_bytes,
            scheduler_load: &t.scheduler_load,
            processes: &t.processes,
        };

        Frame {
            scenario: Some(scenario.id().to_string()),
            running: true,
            uptime_ms,
            ops_per_sec: t.ops_per_sec,
            peak_concurrent,
            profile: self.profile.id().to_string(),
            latency: self.latency.snapshot(),
            throughput: self.throughput.snapshot(),
            observer: self.observer.snapshot(uptime_ms, sample),
        }
    }
}

fn fresh_observer(config: &RunnerConfig, detail_enabled: bool) -> Observer {
    let observer = Observer::new(config.scheduler_count, config.max_detail);
    observer.set_detail_enabled(detail_enabled);
    observer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> Runner {
        Runner::new(RunnerConfig::default())
    }

    #[test]
    fn runner_config_carries_the_tick_rate() {
        let cfg = rusm_node::NodeConfig::from_toml("[node]\nticks_per_second = 30").unwrap();
        assert_eq!(runner_config(&cfg).ticks_per_second, 30);
    }

    #[test]
    fn starts_idle() {
        let r = runner();
        assert!(!r.is_running());
        assert_eq!(r.scenario(), None);
    }

    #[test]
    fn idle_tick_reports_not_running() {
        let mut r = runner();
        let frame = r.tick(10);
        assert!(!frame.running);
        assert_eq!(frame.scenario, None);
        assert_eq!(frame.ops_per_sec, 0.0);
        assert_eq!(frame.uptime_ms, 10);
    }

    #[test]
    fn warmup_excludes_early_latency_then_records_steady_state() {
        // With a warm-up window, the first `warmup_ticks` ticks record no latency
        // (steady-state percentiles aren't inflated by ramp-up); ticks after it do.
        let mut r = Runner::new(RunnerConfig {
            warmup_ticks: 3,
            ..RunnerConfig::default()
        });
        r.start_synthetic(Scenario::PingPong);
        for tick in 0..3 {
            assert_eq!(
                r.tick(tick).latency.count,
                0,
                "tick {tick} is within warm-up — no steady-state latency yet"
            );
        }
        // The 4th tick is past the window: its samples are recorded.
        assert_eq!(
            r.tick(3).latency.count as usize,
            RunnerConfig::default().latency_samples,
            "the first post-warm-up tick records exactly one tick of samples"
        );
    }

    #[test]
    fn start_then_tick_produces_running_frame() {
        let mut r = runner();
        r.start_synthetic(Scenario::DistributedFanout);
        assert!(r.is_running());
        assert_eq!(r.scenario(), Some(Scenario::DistributedFanout));
        let frame = r.tick(50);
        assert!(frame.running);
        assert_eq!(frame.scenario.as_deref(), Some("distributed-fanout"));
        assert!(frame.ops_per_sec > 0.0);
        assert_eq!(
            frame.latency.count as usize,
            RunnerConfig::default().latency_samples
        );
        assert!(frame.observer.messages_total > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn serving_scenarios_sustain_throughput_through_the_runner() {
        // The exact dashboard path: Runner::start → tick, for each serving scenario.
        // A closed-loop driver drives HTTP; the capacity harness drives WS/SSE — all
        // co-resident. We require throughput to be *sustained* (the window stays nonzero),
        // not just to spike once — that's what catches the silent-zero class (e.g. a
        // finite stream held as infinite).
        let mut r = runner();
        for scenario in [
            Scenario::HttpThroughput,
            Scenario::WsEcho,
            Scenario::SseFanout,
        ] {
            r.start(scenario);
            let mut tick = 0u64;
            // Warm-up: wait for first nonzero.
            let mut warmed = false;
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if r.tick(tick).ops_per_sec > 0.0 {
                    warmed = true;
                }
                tick += 1;
                if warmed {
                    break;
                }
            }
            assert!(warmed, "{} never produced throughput", scenario.id());
            // Sustained window: the window must keep producing. We tolerate a single
            // transient dip (CPU starvation when the whole serving suite runs in parallel —
            // many live servers + load generators contend for cores) but fail on **two
            // consecutive** zero ticks: a genuine silent-zero collapse is all-zero, so it
            // still trips. (Same rule as `serving::tests::sustained`.)
            let mut prev_zero = false;
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let ops = r.tick(tick).ops_per_sec;
                tick += 1;
                if ops == 0.0 {
                    assert!(
                        !prev_zero,
                        "{} dropped to 0 while running (silent-zero regression)",
                        scenario.id()
                    );
                    prev_zero = true;
                } else {
                    prev_zero = false;
                }
            }
            r.stop();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repeated_restart_keeps_producing_throughput() {
        // Click like a monkey: run → stop, over and over, for both a bare-process and
        // a WASM-spawning scenario. Every cycle must produce throughput — if a stopped
        // engine leaked its processes/runtime, later cycles would degrade to zero.
        async fn run_until_throughput(r: &mut Runner, scenario: Scenario) -> bool {
            r.start(scenario);
            for tick in 0..400 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if r.tick(tick).ops_per_sec > 0.0 {
                    return true;
                }
            }
            false
        }
        let mut r = runner();
        for scenario in [Scenario::SpawnStorm, Scenario::ComponentStorm] {
            for cycle in 1..=4 {
                assert!(
                    run_until_throughput(&mut r, scenario).await,
                    "{} cycle {cycle} produced throughput",
                    scenario.id()
                );
                r.stop();
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn restart_in_place_releases_prior_engine_before_building_next() {
        // The dashboard "Run" path restarts a scenario WITHOUT stopping first, so `start()`
        // runs over a live serving engine. The fix's invariant — deterministic, independent
        // of load/timing — is that `start()` DROPS the previous run (freeing its WasmRuntime
        // pool reservation) BEFORE building the next, so at most one serving engine is ever
        // live at once. Holding two reservations + the teardown churn is what intermittently
        // left the new run dead-at-zero (the "press Run twice" bug, reproduced live). The
        // per-runner gauge below counts concurrently-live serving engines; restart-in-place
        // must never push it past 1. (`restart_in_place_keeps_warming_serving` then guards
        // the emergent behaviour — that each restart actually warms — end to end.)
        let mut r = runner();
        r.start(Scenario::WsEcho);
        r.start(Scenario::SseFanout); // restart in place, no stop()
        r.start(Scenario::WsEcho);
        assert_eq!(
            r.peak_live_engines(),
            1,
            "drop-before-build: never two serving engines (two pool reservations) at once"
        );
        r.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn restart_in_place_keeps_warming_serving() {
        // End-to-end companion to the invariant test above: the dashboard restart-in-place
        // path (Run without Stop) must keep warming up to live throughput every cycle.
        async fn warm(r: &mut Runner, scenario: Scenario) -> bool {
            r.start(scenario);
            for tick in 0..600 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if r.tick(tick).ops_per_sec > 0.0 {
                    return true;
                }
            }
            false
        }
        let mut r = runner();
        for scenario in [Scenario::WsEcho, Scenario::SseFanout] {
            for cycle in 1..=4 {
                assert!(
                    warm(&mut r, scenario).await,
                    "{} restart-in-place cycle {cycle} warmed up",
                    scenario.id()
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn platform_primitive_scenarios_produce_throughput_through_the_runner() {
        // The dashboard path (Runner::start → tick) for the platform-primitive
        // engines: durable KV, pub/sub fan-out, and TS crypto must each produce real
        // throughput and (for the two that record it) timed latency.
        let mut r = runner();
        for scenario in [
            Scenario::KvStorm,
            Scenario::PubSubFanout,
            Scenario::CryptoOps,
        ] {
            r.start(scenario);
            let mut tick = 0u64;
            let mut produced = false;
            // rquickjs start-up dominates crypto-ops, so poll generously.
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                if r.tick(tick).ops_per_sec > 0.0 {
                    produced = true;
                    break;
                }
                tick += 1;
            }
            assert!(produced, "{} never produced throughput", scenario.id());
            r.stop();
        }
    }

    #[test]
    fn peak_concurrent_is_monotonic_across_ticks() {
        let mut r = runner();
        r.start_synthetic(Scenario::DistributedFanout);
        let mut peak = 0;
        for tick in 0..20 {
            let frame = r.tick(tick);
            assert!(frame.peak_concurrent >= peak);
            peak = frame.peak_concurrent;
        }
    }

    #[test]
    fn stop_returns_to_idle() {
        let mut r = runner();
        r.start_synthetic(Scenario::DistributedFanout);
        r.stop();
        assert!(!r.is_running());
        assert!(!r.tick(0).running);
    }

    #[test]
    fn restart_resets_metrics() {
        let mut r = runner();
        r.start_synthetic(Scenario::DistributedFanout);
        for tick in 0..10 {
            r.tick(tick);
        }
        r.start_synthetic(Scenario::DistributedFanout);
        // A fresh run accumulates from scratch — one tick of synthetic samples.
        let frame = r.tick(0);
        assert_eq!(frame.scenario.as_deref(), Some("distributed-fanout"));
        assert_eq!(
            frame.latency.count as usize,
            RunnerConfig::default().latency_samples
        );
    }

    #[test]
    fn observer_detail_toggle_persists_across_restart() {
        let mut r = runner();
        r.set_observer_detail(false);
        r.start_synthetic(Scenario::DistributedFanout);
        assert!(!r.observer_detail_enabled());
        let frame = r.tick(0);
        assert!(frame.observer.processes.is_empty());
        assert!(frame.observer.process_count > 0);
    }

    #[test]
    fn resource_profile_defaults_to_balanced_and_can_change() {
        let mut r = runner();
        assert_eq!(r.resource_profile(), ResourceProfile::Balanced);
        r.set_resource_profile(ResourceProfile::Light);
        assert_eq!(r.resource_profile(), ResourceProfile::Light);
        // Reflected in the frame.
        assert_eq!(r.tick(0).profile, "light");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_storm_runs_real_processes() {
        // The spawn-storm scenario uses the real rusm-otp engine (continuous
        // background spawners, needs a Tokio runtime), unlike the synthetic ones.
        let mut r = runner();
        r.start(Scenario::SpawnStorm);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await; // warm up
        let frame = r.tick(50);
        assert_eq!(frame.scenario.as_deref(), Some("spawn-storm"));
        assert!(frame.ops_per_sec > 0.0);
        assert!(frame.latency.count > 0);

        // Changing the profile mid-run restarts the storm with the new tuning.
        r.set_resource_profile(ResourceProfile::Max);
        assert_eq!(r.scenario(), Some(Scenario::SpawnStorm));
        assert_eq!(r.resource_profile(), ResourceProfile::Max);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ping_pong_runs_real_processes() {
        // The ping-pong scenario bounces messages between real rusm-otp process
        // pairs, unlike the synthetic scenarios.
        let mut r = runner();
        r.start(Scenario::PingPong);
        tokio::time::sleep(std::time::Duration::from_millis(80)).await; // warm up
        let frame = r.tick(50);
        assert_eq!(frame.scenario.as_deref(), Some("ping-pong"));
        assert!(frame.ops_per_sec > 0.0);
        assert!(frame.observer.process_count >= 2); // at least one live pair
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fault_recovery_runs_real_supervisors() {
        // Real supervisors trap exits and restart crashing children.
        let mut r = runner();
        r.start(Scenario::FaultRecovery);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await; // warm up
        let frame = r.tick(50);
        assert_eq!(frame.scenario.as_deref(), Some("fault-recovery"));
        assert!(frame.ops_per_sec > 0.0); // restarts/sec
        assert!(frame.observer.process_count >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connection_storm_runs_real_tcp() {
        // Real loopback TCP, one process per accepted connection.
        let mut r = runner();
        r.start(Scenario::ConnectionStorm);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await; // ramp up
        let frame = r.tick(50);
        assert_eq!(frame.scenario.as_deref(), Some("connection-storm"));
        assert!(frame.ops_per_sec > 0.0); // connections/sec
        assert!(frame.observer.process_count > 1); // live connections + acceptor
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fairness_runs_real_wasm_and_bystanders_progress() {
        // Real Wasm spinners + bystanders; epoch preemption keeps bystanders going.
        // Poll until progress shows (robust to scheduling/parallel-test load).
        let mut r = runner();
        r.start(Scenario::Fairness);
        let mut frame = r.tick(0);
        let mut progressed = false;
        for tick in 1..=200 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            frame = r.tick(tick);
            if frame.ops_per_sec > 0.0 {
                progressed = true;
                break;
            }
        }
        assert_eq!(frame.scenario.as_deref(), Some("fairness"));
        assert!(progressed, "bystanders progressed despite spinners");
        assert!(frame.observer.process_count >= 2);
    }

    #[test]
    fn throughput_window_is_bounded() {
        let mut r = runner();
        r.start_synthetic(Scenario::DistributedFanout);
        for tick in 0..500 {
            r.tick(tick);
        }
        let frame = r.tick(500);
        assert!(frame.throughput.points.len() <= RunnerConfig::default().throughput_window);
    }
}
