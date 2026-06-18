//! **SSE stress benchmark** — hold many concurrent `text/event-stream` connections
//! against a WASM component streaming events as fast as each client drains them.
//! Reports total events/sec, concurrent streams held, and stream-setup p50/p99 — the
//! "many long-lived streaming responses" story (where a NATS-lattice runtime tends
//! to wobble). Each stream is one component instance; a dropped client tears down
//! only its own instance, never the others or the listener.
//!
//! ```sh
//! cargo run --release -p rusm-bench --example sse_bench -- [seconds] [streams]
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusm_otp::Runtime;
use rusm_wasm::{CapabilityProfile, WasmRuntime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// An endless `wasi:http` SSE stream — yields events as fast as the client reads.
const FIREHOSE: &[u8] =
    include_bytes!("../../../crates/rusm-wasm/tests/fixtures/sse_firehose.wasm");

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: u64 = arg(1).unwrap_or(5);
    let streams: usize = arg(2).unwrap_or(128);
    println!("SSE stress: {streams} concurrent event streams, {secs}s\n");

    let wr = WasmRuntime::new(Runtime::new())?;
    let prepared = wr.prepare_http(&wr.compile_component(FIREHOSE)?)?;
    let server = wr.http_server(&prepared, CapabilityProfile::Trusted.capabilities());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(server.serve(listener));

    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(AtomicU64::new(0));
    let alive = Arc::new(AtomicU64::new(0));
    let setup = Arc::new(Mutex::new(Vec::<u64>::new()));

    let clients: Vec<_> = (0..streams)
        .map(|_| {
            let (stop, events, alive, setup) =
                (stop.clone(), events.clone(), alive.clone(), setup.clone());
            tokio::spawn(async move {
                let opened = Instant::now();
                let Ok(mut conn) = TcpStream::connect(addr).await else {
                    return;
                };
                conn.set_nodelay(true).ok();
                if conn
                    .write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                alive.fetch_add(1, Ordering::Relaxed);

                // Count SSE events by their `\n\n` frame terminators, carrying the last
                // byte across read boundaries so a split terminator still counts once.
                let mut buf = [0u8; 16 * 1024];
                let mut prev_was_newline = false;
                let mut first_seen = false;
                while !stop.load(Ordering::Relaxed) {
                    let Ok(n) = conn.read(&mut buf).await else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    if !first_seen {
                        setup
                            .lock()
                            .unwrap()
                            .push(opened.elapsed().as_nanos() as u64);
                        first_seen = true;
                    }
                    let mut count = 0u64;
                    for &b in &buf[..n] {
                        if b == b'\n' {
                            if prev_was_newline {
                                count += 1;
                                prev_was_newline = false;
                            } else {
                                prev_was_newline = true;
                            }
                        } else {
                            prev_was_newline = false;
                        }
                    }
                    events.fetch_add(count, Ordering::Relaxed);
                }
            })
        })
        .collect();

    let start = Instant::now();
    tokio::time::sleep(Duration::from_secs(secs)).await;
    let held = alive.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    for client in clients {
        let _ = client.await;
    }
    task.abort();

    let elapsed = start.elapsed().as_secs_f64();
    let rate = events.load(Ordering::Relaxed) as f64 / elapsed;
    let mut s = setup.lock().unwrap().clone();
    s.sort_unstable();
    let pct = |p: f64| -> f64 {
        if s.is_empty() {
            0.0
        } else {
            s[((s.len() - 1) as f64 * p) as usize] as f64 / 1000.0
        }
    };

    println!("streams held:  {held}/{streams}   (each its own component instance)");
    println!(
        "events:        {rate:.0}/sec total   ({:.0}/sec per stream)",
        rate / held.max(1) as f64
    );
    println!(
        "stream setup:  p50 {:.1}µs  p99 {:.1}µs   (connect → first event)",
        pct(0.50),
        pct(0.99)
    );
    Ok(())
}

fn arg<T: std::str::FromStr>(n: usize) -> Option<T> {
    std::env::args().nth(n).and_then(|s| s.parse().ok())
}
