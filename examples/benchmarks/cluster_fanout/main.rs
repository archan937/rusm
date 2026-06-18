//! **Cluster fan-out benchmark** — real cross-node throughput and latency over the
//! QUIC + TLS transport, measured (not synthetic).
//!
//! Run it (release, for real numbers):
//! ```sh
//! cargo run --release -p rusm-bench --example cluster_fanout -- [seconds] [worker-nodes]
//! ```
//!
//! Topology: one **hub** node and N **worker** nodes (all in this process, each on
//! its own loopback QUIC endpoint — a faithful stand-in for separate machines).
//! Every worker runs an `echo` process that bounces a message back to a named
//! process on the hub. The message carries `[8-byte send-time][reply-process-name]`,
//! so the same echo serves two measurements:
//!
//!   1. **Latency** — one in-flight round-trip at a time against a `probe` process,
//!      giving the *unloaded* round-trip time (no queueing).
//!   2. **Throughput** — a pool of senders saturating the links, counting completed
//!      round-trips at a `collector` process (each round-trip = two cross-node hops).
//!
//! Measuring latency separately matters: under saturation, latency is dominated by
//! queue depth, not the transport — so a single number for both would mislead.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusm_cluster::{ClusterNode, Identity};
use rusm_otp::Runtime;
use tokio::sync::mpsc::unbounded_channel;

/// Unloaded round-trips to time for the latency figure.
const LATENCY_PROBES: usize = 2000;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: u64 = arg(1).unwrap_or(5);
    let workers: usize = arg(2).unwrap_or(4).max(1);
    let senders = (workers * 8).max(8);

    let local: SocketAddr = "127.0.0.1:0".parse()?;
    let id = Identity::generate()?;
    let base = Instant::now();

    // Hub. The `collector` counts saturation round-trips; the `probe` signals each
    // unloaded round-trip back to the latency loop over a channel.
    let hub = ClusterNode::bind("hub", Runtime::new(), local, &id)?;
    let delivered = Arc::new(AtomicU64::new(0));
    {
        let delivered = delivered.clone();
        let collector = hub.runtime().spawn(move |mut ctx| async move {
            while ctx.recv().await.message().is_some() {
                delivered.fetch_add(1, Ordering::Relaxed);
            }
        });
        hub.runtime().register("collector", collector.pid());
    }
    let (probe_tx, mut probe_rx) = unbounded_channel();
    {
        let probe = hub.runtime().spawn(move |mut ctx| async move {
            while ctx.recv().await.message().is_some() {
                let _ = probe_tx.send(());
            }
        });
        hub.runtime().register("probe", probe.pid());
    }

    // Worker nodes: each bounces a message back to the hub process it names.
    let mut nodes = vec![hub.clone()];
    for i in 0..workers {
        let worker = ClusterNode::bind(format!("w{i}"), Runtime::new(), local, &id)?;
        let echo_node = worker.clone();
        let echo = worker.runtime().spawn(move |mut ctx| async move {
            while let Some(m) = ctx.recv().await.message() {
                if m.len() >= 8 {
                    let reply = std::str::from_utf8(&m[8..]).unwrap_or("collector");
                    let _ = echo_node.send("hub", reply, &m).await;
                }
            }
        });
        worker.runtime().register("echo", echo.pid());
        hub.connect(worker.local_addr()?).await?;
        nodes.push(worker);
    }

    // --- Phase 1: unloaded round-trip latency (one in flight). ---
    let mut rtts = Vec::with_capacity(LATENCY_PROBES);
    for _ in 0..LATENCY_PROBES {
        let start = Instant::now();
        hub.send("w0", "echo", &message(base, "probe")).await?;
        probe_rx.recv().await;
        rtts.push(start.elapsed().as_nanos() as u64);
    }
    rtts.sort_unstable();
    let pct = |p: f64| rtts[((rtts.len() - 1) as f64 * p) as usize] as f64 / 1000.0;

    // --- Phase 2: saturation throughput (many senders, fire-and-forget). ---
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    for s in 0..senders {
        let hub = hub.clone();
        let worker = format!("w{}", s % workers);
        let stop = stop.clone();
        handles.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                if hub
                    .send(&worker, "echo", &message(base, "collector"))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    delivered.store(0, Ordering::Relaxed);
    let throughput_start = Instant::now();
    tokio::time::sleep(Duration::from_secs(secs)).await;
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }
    let elapsed = throughput_start.elapsed().as_secs_f64();
    let round_trips = delivered.load(Ordering::Relaxed);
    let rate = round_trips as f64 / elapsed;

    println!("cluster fan-out: {workers} worker nodes, {senders} senders (QUIC+TLS loopback)\n");
    println!(
        "unloaded round-trip latency: p50 {:.1}µs  p99 {:.1}µs",
        pct(0.50),
        pct(0.99)
    );
    println!("saturation round-trips:      {round_trips}  ({rate:.0}/sec)");
    println!(
        "saturation cross-node msgs:  {}  ({:.0}/sec)",
        round_trips * 2,
        rate * 2.0
    );
    let _ = nodes; // held alive for the run
    Ok(())
}

/// `[8-byte send-time][reply-process-name]` — the echo reads the name and bounces
/// the whole payload back to that process on the hub.
fn message(base: Instant, reply: &str) -> Vec<u8> {
    let ts = base.elapsed().as_nanos() as u64;
    let mut v = ts.to_le_bytes().to_vec();
    v.extend_from_slice(reply.as_bytes());
    v
}

fn arg<T: std::str::FromStr>(n: usize) -> Option<T> {
    std::env::args().nth(n).and_then(|s| s.parse().ok())
}
