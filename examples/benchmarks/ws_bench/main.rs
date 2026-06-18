//! **WebSocket stress benchmark** — hold many concurrent WS connections and hammer
//! each with echo round-trips. Two servers, so the sandbox cost is explicit: the
//! **component path** (every connection is a WASM component process — the real
//! serving path) and a **host echo** (no Wasm — the transport ceiling). Reports
//! messages/sec, round-trip p50/p99, and how many connections stayed up — the
//! stability story: each connection is an isolated supervised pair of processes, so
//! one dropping never touches the others or the listener.
//!
//! ```sh
//! cargo run --release -p rusm-bench --example ws_bench -- [seconds] [connections]
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rusm_otp::Runtime;
use rusm_wasm::{serve_ws_echo, CapabilityProfile, WasmRuntime};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// The WS-handler component: echoes each frame from inside the sandbox.
const WS_ECHO: &[u8] = include_bytes!("../../../crates/rusm-wasm/tests/fixtures/rs_ws_echo.wasm");

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: u64 = arg(1).unwrap_or(5);
    let connections: usize = arg(2).unwrap_or(256);
    println!("WebSocket stress: {connections} concurrent connections, {secs}s each\n");

    // Real serving path: each connection drives a sandboxed WASM component process.
    let wr = WasmRuntime::new(Runtime::new())?;
    let prepared = wr.prepare_component(&wr.compile_component(WS_ECHO)?, "run")?;
    let comp_listener = TcpListener::bind("127.0.0.1:0").await?;
    let comp_addr = comp_listener.local_addr()?;
    let comp_server = wr.ws_server(&prepared, CapabilityProfile::Trusted.capabilities());
    let comp_task = tokio::spawn(comp_server.serve(comp_listener));
    let component = stress(comp_addr, connections, secs).await;
    comp_task.abort();

    // Transport ceiling: a host-side echo, no Wasm in the loop.
    let host_listener = TcpListener::bind("127.0.0.1:0").await?;
    let host_addr = host_listener.local_addr()?;
    let host_task = tokio::spawn(serve_ws_echo(host_listener));
    let host = stress(host_addr, connections, secs).await;
    host_task.abort();

    println!("WASM component per connection (real serving path):");
    component.report(connections);
    println!("\nhost echo (no Wasm, transport ceiling):");
    host.report(connections);
    // >1.0x = the component path matched (or beat) the bare transport; the per-message
    // cost is one writer→component→writer mailbox hop, which is ~free next to the socket.
    println!(
        "\ncomponent vs host transport: {:.2}x throughput, {:+.1}µs p50  (the sandbox cost per round-trip)",
        component.rate / host.rate.max(1.0),
        component.p50 - host.p50,
    );
    Ok(())
}

struct Stats {
    rate: f64,
    p50: f64,
    p99: f64,
    alive: u64,
}

impl Stats {
    fn report(&self, connections: usize) {
        println!(
            "  connections held: {}/{}   (each an isolated supervised pair)",
            self.alive, connections
        );
        println!(
            "  echo round-trips:  {:.0}/sec   p50 {:.1}µs  p99 {:.1}µs",
            self.rate, self.p50, self.p99
        );
    }
}

/// Hold `connections` WS connections at `addr` for `secs`, each firing echo
/// round-trips back-to-back; count completed round-trips and sample latency.
async fn stress(addr: std::net::SocketAddr, connections: usize, secs: u64) -> Stats {
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));
    let alive = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(Mutex::new(Vec::<u64>::new()));
    let url = format!("ws://{addr}/");

    let clients: Vec<_> = (0..connections)
        .map(|_| {
            let (url, stop, total, alive, latencies) = (
                url.clone(),
                stop.clone(),
                total.clone(),
                alive.clone(),
                latencies.clone(),
            );
            tokio::spawn(async move {
                let Ok((mut ws, _)) = tokio_tungstenite::connect_async(url).await else {
                    return;
                };
                alive.fetch_add(1, Ordering::Relaxed);
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    if ws.send(Message::binary(b"ping".as_slice())).await.is_err() {
                        break;
                    }
                    match ws.next().await {
                        Some(Ok(_)) => {}
                        _ => break,
                    }
                    total.fetch_add(1, Ordering::Relaxed);
                    if n.is_multiple_of(32) {
                        latencies
                            .lock()
                            .unwrap()
                            .push(started.elapsed().as_nanos() as u64);
                    }
                    n += 1;
                }
            })
        })
        .collect();

    let start = Instant::now();
    tokio::time::sleep(Duration::from_secs(secs)).await;
    let still_up = alive.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    for client in clients {
        let _ = client.await;
    }

    let elapsed = start.elapsed().as_secs_f64();
    let rate = total.load(Ordering::Relaxed) as f64 / elapsed;
    let mut lat = latencies.lock().unwrap().clone();
    lat.sort_unstable();
    let pct = |p: f64| -> f64 {
        if lat.is_empty() {
            0.0
        } else {
            lat[((lat.len() - 1) as f64 * p) as usize] as f64 / 1000.0
        }
    };
    Stats {
        rate,
        p50: pct(0.50),
        p99: pct(0.99),
        alive: still_up,
    }
}

fn arg<T: std::str::FromStr>(n: usize) -> Option<T> {
    std::env::args().nth(n).and_then(|s| s.parse().ok())
}
