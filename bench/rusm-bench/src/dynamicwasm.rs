use std::time::Instant;

use futures_util::stream::{FuturesUnordered, StreamExt};
use rusm_otp::Runtime;
use rusm_wasm::WasmRuntime;
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::task::JoinHandle;

use crate::sample::Sample;

/// Record a spawn's latency every Nth op, bounding the sample stream.
const LATENCY_EVERY: u64 = 16;
/// Most latency samples surfaced in a single tick.
const LATENCY_SAMPLE: usize = 64;
/// Target total live component instances across all workers — kept below the pooling
/// allocator's slot count so a spawn never exhausts the pool. Same bound as the component
/// storm, so the two rates are directly comparable.
const MAX_LIVE: usize = 100;

/// The runtime-loaded component: the same minimal shape as the component storm (one page of
/// memory + a `run` that returns), but reached the **dynamic** way — by content-addressed
/// source, not a pre-prepared handle. Compiled **once** on the first spawn (cold), then
/// served from the cache for every later spawn (hot).
const COMPONENT: &str = r#"(component
    (core module $m (memory (export "mem") 1) (func (export "run")))
    (core instance $i (instantiate $m))
    (func (export "run") (canon lift (core func $i "run"))))"#;

/// A **real, continuous dynamic-WASM spawn storm**: background workers spawn components
/// through the content-addressed compile cache — `prepare_dynamic(source)` then
/// `spawn_component` — exactly the path a guest's `spawn-from "kv:…"` takes. The very first
/// spawn compiles the bundle (cold); every later spawn is a cache hit instantiating on the
/// pooled fast path (hot). [`tick`](Self::tick) reports the achieved **hot** rate (Δspawned
/// / Δt) plus per-spawn latency (the first, cold, sample shows the one-time compile cost).
///
/// The point it proves: runtime-chosen compiled components spawn at essentially the same
/// rate as built-in ones, because the compile is paid once and cached. Must be constructed
/// inside a Tokio runtime (it starts the Wasm epoch ticker).
pub struct DynamicWasmEngine {
    runtime: Runtime,
    // Owns the Wasm engine + epoch ticker, shared with the workers.
    _wasm: Arc<WasmRuntime>,
    workers: Vec<JoinHandle<()>>,
    latency_rx: UnboundedReceiver<u64>,
    last_spawned: u64,
    last_at: Instant,
    scheduler_count: usize,
}

impl DynamicWasmEngine {
    pub fn new(workers: usize, scheduler_count: usize) -> Self {
        let runtime = Runtime::new();
        let wasm = Arc::new(WasmRuntime::new(runtime.clone()).expect("wasm engine"));
        let (latency_tx, latency_rx) = unbounded_channel();

        // Each worker keeps a bounded set of in-flight components and parks on their
        // completion (the await is the backpressure — no busy-spin). Every iteration goes
        // through the cache: one source, so the first call compiles and the rest hit.
        let worker_count = workers.max(1);
        let per_worker = (MAX_LIVE / worker_count).max(1);
        let source = format!("inline:{COMPONENT}");
        let workers = (0..worker_count)
            .map(|_| {
                let wasm = Arc::clone(&wasm);
                let latency_tx = latency_tx.clone();
                let source = source.clone();
                tokio::spawn(async move {
                    let mut inflight = FuturesUnordered::new();
                    let mut round: u64 = 0;
                    loop {
                        while inflight.len() < per_worker {
                            let started = Instant::now();
                            let prepared = match wasm.prepare_dynamic(&source, "run").await {
                                Ok(prepared) => prepared,
                                Err(_) => return, // engine torn down
                            };
                            let handle = wasm.spawn_component(prepared.as_ref());
                            if round.is_multiple_of(LATENCY_EVERY) {
                                let _ = latency_tx.send(started.elapsed().as_nanos() as u64);
                            }
                            round += 1;
                            inflight.push(async move { handle.join().await });
                        }
                        inflight.next().await; // park until one finishes, then refill
                    }
                })
            })
            .collect();

        Self {
            runtime,
            _wasm: wasm,
            workers,
            latency_rx,
            last_spawned: 0,
            last_at: Instant::now(),
            scheduler_count,
        }
    }

    pub fn tick(&mut self) -> Sample {
        let now = Instant::now();
        let spawned = self.runtime.spawned();
        let dt = now
            .duration_since(self.last_at)
            .as_secs_f64()
            .max(f64::MIN_POSITIVE);
        let ops_per_sec = spawned.saturating_sub(self.last_spawned) as f64 / dt;
        self.last_spawned = spawned;
        self.last_at = now;

        // Latency is measured in the workers (the cache get is async), drained here.
        let mut latencies_ns = Vec::new();
        while let Ok(ns) = self.latency_rx.try_recv() {
            latencies_ns.push(ns);
        }
        if latencies_ns.len() > LATENCY_SAMPLE {
            latencies_ns = latencies_ns.split_off(latencies_ns.len() - LATENCY_SAMPLE);
        }

        let process_count = self.runtime.process_count() as u64;
        Sample {
            ops_per_sec,
            process_count,
            running: process_count,
            waiting: 0,
            total_memory_bytes: 0,
            latencies_ns,
            processes: Vec::new(),
            scheduler_load: vec![0.0; self.scheduler_count],
        }
    }
}

impl Drop for DynamicWasmEngine {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.abort();
        }
        // Catch-all: abort every component still on the runtime so none outlive the engine.
        self.runtime.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dynamic_wasm_spawns_continuously_through_the_cache() {
        let mut engine = DynamicWasmEngine::new(4, 4);
        // Warm up past the cold compile (slow in a debug build, slower under the parallel
        // bench-test load), then measure. Poll rather than sleep-once so the test is robust
        // to machine load — it asserts the steady hot state, however long warm-up takes.
        let mut sample = engine.tick();
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            sample = engine.tick();
            if sample.ops_per_sec > 0.0 && !sample.latencies_ns.is_empty() {
                break;
            }
        }
        assert!(sample.ops_per_sec > 0.0, "components should be spawning");
        assert_eq!(sample.scheduler_load.len(), 4);
        assert!(
            !sample.latencies_ns.is_empty(),
            "dynamic spawns should be timed"
        );
        assert!(engine.runtime.spawned() > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn live_population_stays_bounded() {
        let mut engine = DynamicWasmEngine::new(4, 2);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(engine.tick().process_count < 256);
    }
}
