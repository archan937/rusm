//! **HTTP stress benchmark** — real throughput + latency of serving a WASM
//! component as an HTTP handler. Three servers, so each cost is explicit: a **lean**
//! raw-`wasi:http` component, the ergonomic **wstd** component (async reactor per
//! request), and a **bare-hyper baseline** (no Wasm). The lean-vs-wstd gap is the
//! guest's cost; the lean-vs-baseline gap is the true sandbox cost.
//!
//! ```sh
//! cargo run --release -p rusm-bench --example http_bench -- [seconds] [clients]
//! ```
//!
//! Each client holds one keep-alive connection and fires requests back-to-back; we
//! report sustained requests/sec, per-request p50/p99, and instantiate-only cost.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use rusm_otp::Runtime;
use rusm_wasm::{CapabilityProfile, WasmRuntime};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// wstd `wasi:http` component (ergonomic, carries an async reactor).
const WSTD: &[u8] = include_bytes!("../../../crates/rusm-wasm/tests/fixtures/http_hello.wasm");
/// Lean `wasi:http` component (raw bindings, no reactor) — the host's serving ceiling.
const LEAN: &[u8] = include_bytes!("../../../crates/rusm-wasm/tests/fixtures/http_lean.wasm");

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: u64 = arg(1).unwrap_or(5);
    let clients: usize = arg(2).unwrap_or(64);
    println!("HTTP stress: {clients} keep-alive clients, {secs}s each\n");

    // The guest you write determines the cost: a lean raw-wasi:http component vs the
    // ergonomic wstd one (which runs an async reactor per request).
    let (lean, lean_inst) = wasm_run(LEAN, clients, secs).await?;
    let (wstd, wstd_inst) = wasm_run(WSTD, clients, secs).await?;

    // Bare-hyper baseline: identical loop, a static response, no Wasm at all.
    let base_listener = TcpListener::bind("127.0.0.1:0").await?;
    let base_addr = base_listener.local_addr()?;
    let base_task = tokio::spawn(serve_baseline(base_listener));
    let base = stress(base_addr, clients, secs).await;
    base_task.abort();

    println!("lean WASM component (raw wasi:http):");
    lean.report();
    println!(
        "  instantiate-only: {lean_inst:.0}/sec = {:.1}µs each",
        1e6 / lean_inst
    );
    println!("\nwstd WASM component (async reactor per request):");
    wstd.report();
    println!(
        "  instantiate-only: {wstd_inst:.0}/sec = {:.1}µs each",
        1e6 / wstd_inst
    );

    println!("\nbare hyper (no Wasm, baseline):");
    base.report();
    println!(
        "\nlean WASM vs bare hyper: {:.1}x fewer req/s, +{:.1}µs p50  (the true sandbox cost)",
        base.rps / lean.rps.max(1.0),
        lean.p50 - base.p50,
    );
    Ok(())
}

/// Serve `component` over HTTP, stress it, and also time instantiation alone.
async fn wasm_run(
    component: &[u8],
    clients: usize,
    secs: u64,
) -> Result<(Stats, f64), Box<dyn std::error::Error>> {
    let wr = WasmRuntime::new(Runtime::new())?;
    let prepared = wr.prepare_http(&wr.compile_component(component)?)?;
    let server = wr.http_server(&prepared, CapabilityProfile::Trusted.capabilities());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(server.serve(listener));
    let stats = stress(addr, clients, secs).await;
    task.abort();

    // Instantiation-only (1 thread): the per-request sandbox cost in isolation.
    let inst = wr.http_server(&prepared, CapabilityProfile::Trusted.capabilities());
    let mut n = 0u64;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        inst.instantiate_once().await?;
        n += 1;
    }
    Ok((stats, n as f64 / start.elapsed().as_secs_f64()))
}

struct Stats {
    rps: f64,
    p50: f64,
    p99: f64,
}

impl Stats {
    fn report(&self) {
        println!(
            "  {:.0} req/sec   latency p50 {:.1}µs  p99 {:.1}µs",
            self.rps, self.p50, self.p99
        );
    }
}

/// Drive `clients` keep-alive connections at `addr` for `secs`, counting completed
/// requests and sampling latency.
async fn stress(addr: SocketAddr, clients: usize, secs: u64) -> Stats {
    let stop = Arc::new(AtomicBool::new(false));
    let total = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(Mutex::new(Vec::<u64>::new()));

    let tasks: Vec<_> = (0..clients)
        .map(|_| {
            let (stop, total, latencies) = (stop.clone(), total.clone(), latencies.clone());
            tokio::spawn(async move {
                let Ok(conn) = TcpStream::connect(addr).await else {
                    return;
                };
                conn.set_nodelay(true).ok();
                let mut reader = BufReader::new(conn);
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    if request(&mut reader).await.is_err() {
                        break;
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
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        let _ = task.await;
    }

    let elapsed = start.elapsed().as_secs_f64();
    let rps = total.load(Ordering::Relaxed) as f64 / elapsed;
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
        rps,
        p50: pct(0.50),
        p99: pct(0.99),
    }
}

/// One keep-alive HTTP/1.1 request + a full response read (Content-Length *or*
/// chunked), so the connection stays in sync for the next request.
async fn request(reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
    reader
        .get_mut()
        .write_all(b"GET / HTTP/1.1\r\nHost: rusm\r\n\r\n")
        .await?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Err(eof());
        }
        let header = line.trim_end();
        if header.is_empty() {
            break; // end of headers
        }
        let lower = header.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }

    if chunked {
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                return Err(eof());
            }
            let size = usize::from_str_radix(line.trim_end(), 16).unwrap_or(0);
            // chunk data + its trailing CRLF (for the 0-chunk, just the terminator)
            let mut chunk = vec![0u8; size + 2];
            reader.read_exact(&mut chunk).await?;
            if size == 0 {
                break;
            }
        }
    } else {
        let mut body = vec![0u8; content_length.unwrap_or(0)];
        reader.read_exact(&mut body).await?;
    }
    Ok(())
}

fn eof() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed")
}

/// A bare hyper server returning a static body — the no-Wasm baseline.
async fn serve_baseline(listener: TcpListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        stream.set_nodelay(true).ok();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(|_req| async {
                Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from_static(
                    b"hello from RUSM\n",
                ))))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .keep_alive(true)
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

fn arg<T: std::str::FromStr>(n: usize) -> Option<T> {
    std::env::args().nth(n).and_then(|s| s.parse().ok())
}
