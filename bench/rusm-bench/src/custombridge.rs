use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rusm_otp::{ProcessHandle, Runtime};
use rusm_wasm::{BridgeHost, WasmRuntime};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::sample::Sample;

/// Record a bridge round-trip's latency every Nth op, bounding the sample stream.
const LATENCY_EVERY: u64 = 16;
/// Most latency samples surfaced in a single tick.
const LATENCY_SAMPLE: usize = 64;

/// The benchmark guest: a sandboxed component that calls the custom `demo:bridge/greet`
/// host function in a loop (one call per request). Built from
/// `tests/fixtures/bench-bridge`.
const GUEST: &[u8] = include_bytes!("../../../crates/rusm-wasm/tests/fixtures/bench_bridge.wasm");

/// Host bindings for the custom application bridge the guest imports — the same
/// `demo:bridge/greet` contract an app declares in `bridges/greet/bridge.wit`, implemented
/// here as a native host function. This is the bench's stand-in for an app's
/// `bridges/<name>/host.rs`: a typed WIT function backed by host Rust, reaching real
/// process state (the calling pid) through the curated [`BridgeHost`] accessor.
mod greet_bridge {
    wasmtime::component::bindgen!({
        inline: "
            package demo:bridge@0.1.0;
            interface greet { greet: func(name: string) -> string; }
            world greet-host { import greet; }
        ",
        imports: { default: async },
    });
}

impl greet_bridge::demo::bridge::greet::Host for BridgeHost {
    async fn greet(&mut self, name: String) -> String {
        format!("hello, {name} from {}", self.pid())
    }
}

/// A **real, continuous custom-bridge storm**: `instances` sandboxed components each call a
/// native host bridge (`demo:bridge/greet`, wired via [`WasmRuntime::with_bridges`]) in a
/// loop, while a Rust driver keeps one request in flight per guest and counts the typed
/// replies. [`tick`](Self::tick) reports native-bridge round-trips/sec and the per-call
/// latency.
///
/// The number is the honest rate at which a sandboxed guest can call an app-defined native
/// function — it includes the typed WIT call and the message round-trip, the true cost of a
/// custom bridge (RUSM's compiled-in answer to a capability provider). Must be constructed
/// inside a Tokio runtime.
pub struct CustomBridgeEngine {
    runtime: Runtime,
    // Owns the Wasm engine (with the custom bridge linked) + epoch ticker.
    _wasm: Arc<WasmRuntime>,
    processes: Vec<ProcessHandle>,
    ops: Arc<AtomicU64>,
    latency_rx: UnboundedReceiver<u64>,
    last_ops: u64,
    last_at: Instant,
    scheduler_count: usize,
}

impl CustomBridgeEngine {
    pub fn new(instances: usize, scheduler_count: usize) -> Self {
        let runtime = Runtime::new();
        let wasm = Arc::new(
            WasmRuntime::with_bridges(runtime.clone(), |linker| {
                greet_bridge::demo::bridge::greet::add_to_linker::<
                    _,
                    wasmtime::component::HasSelf<BridgeHost>,
                >(linker, |host| host)
            })
            .expect("wasm engine with the custom bridge"),
        );
        let prepared = wasm
            .prepare_component(&wasm.compile_component(GUEST).expect("compile"), "run")
            .expect("prepare");

        let ops = Arc::new(AtomicU64::new(0));
        let (latency_tx, latency_rx) = unbounded_channel();
        let mut processes = Vec::new();

        for _ in 0..instances.max(1) {
            let guest = wasm.spawn_component(&prepared);
            let guest_pid = guest.pid();
            processes.push(guest);

            // Driver: hand the guest our pid, then loop request → reply, counting and
            // timing. One request in flight (the bridge round-trip is the unit).
            let driver_rt = runtime.clone();
            let ops = Arc::clone(&ops);
            let latency_tx = latency_tx.clone();
            let driver = runtime.spawn(move |mut ctx| async move {
                driver_rt.send(guest_pid, ctx.pid().raw().to_string().into_bytes());
                let mut round: u64 = 0;
                loop {
                    let started = Instant::now();
                    driver_rt.send(guest_pid, b"go".to_vec());
                    let _ = ctx.recv().await; // the host's typed greet reply
                    ops.fetch_add(1, Ordering::Relaxed);
                    round += 1;
                    if round.is_multiple_of(LATENCY_EVERY) {
                        let _ = latency_tx.send(started.elapsed().as_nanos() as u64);
                    }
                }
            });
            processes.push(driver);
        }

        Self {
            runtime,
            _wasm: wasm,
            processes,
            ops,
            latency_rx,
            last_ops: 0,
            last_at: Instant::now(),
            scheduler_count,
        }
    }

    pub fn tick(&mut self) -> Sample {
        let now = Instant::now();
        let ops = self.ops.load(Ordering::Relaxed);
        let dt = now
            .duration_since(self.last_at)
            .as_secs_f64()
            .max(f64::MIN_POSITIVE);
        let ops_per_sec = ops.saturating_sub(self.last_ops) as f64 / dt;
        self.last_ops = ops;
        self.last_at = now;

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

impl Drop for CustomBridgeEngine {
    fn drop(&mut self) {
        for process in &self.processes {
            process.kill();
        }
        // Catch-all: tear down any component still on the runtime.
        self.runtime.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn guests_call_the_native_bridge_and_report_rate_and_latency() {
        let mut engine = CustomBridgeEngine::new(2, 4);
        // Warm-up so bridge calls flow and samples surface.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let sample = engine.tick();
        assert!(sample.ops_per_sec > 0.0, "bridge calls should be flowing");
        assert!(sample.process_count >= 2, "guests + drivers are alive");
        assert_eq!(sample.scheduler_load.len(), 4);
        assert!(
            !sample.latencies_ns.is_empty(),
            "bridge round-trips should be timed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn instance_count_is_at_least_one() {
        let engine = CustomBridgeEngine::new(0, 1);
        assert_eq!(engine.processes.len(), 2); // one guest + one driver
    }
}
