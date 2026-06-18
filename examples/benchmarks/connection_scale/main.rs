//! **Connection scale** — how many concurrent connections can RUSM *hold at once*,
//! each served by its own isolated `rusm-otp` process? This pushes to the machine's
//! ceiling and reports the measured peak.
//!
//! Unlike `connection-storm` (which recycles connections to measure connect/sec
//! throughput against **one** port), this **holds every connection open**, so the
//! question is pure concurrency. It ramps to a target (default: 80% of the system fd
//! ceiling) and reports the peak held, stopping when held plateaus.
//!
//! The ceiling is the **OS**, never RUSM: minting a process per connection is near-free.
//! Two kernel walls, both lifted to expose the real one:
//! - **File descriptors** — loopback puts both ends in this process, so a connection
//!   costs 2 fds. We raise the limit to the per-process cap; that's the ceiling.
//! - **Ephemeral source ports** — a naive loopback client exhausts its ~16k ephemeral
//!   ports first. We dodge that with the *4-tuple trick*: each client task owns a
//!   disjoint stripe of explicit source ports (`SO_REUSEADDR`), each paired with all
//!   `PORTS` destinations, so every `(src, dst)` 4-tuple is unique and no task races
//!   another to bind a port. Socket buffers are shrunk so RAM isn't the wall.
//!
//! ```sh
//! cargo run --release -p rusm-bench --example connection_scale -- [target] [client_tasks]
//! ```

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusm_otp::Runtime;
use socket2::{Domain, Socket, Type};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

/// Listener ports to shard destinations across. With the source-port trick below,
/// each of these multiplies the usable 4-tuple space, so the ephemeral range stops
/// being the wall — the fd ceiling is.
const PORTS: usize = 34;
/// Client source ports start here (below the 49152+ ephemeral range the listeners
/// land in, so they never collide). Each source port pairs with all `PORTS`
/// destinations → `PORTS` unique connections per source port.
const SRC_BASE: u16 = 20_000;
/// Small socket buffers: held-idle connections move no data, so this just trims the
/// kernel memory per socket.
const SOCK_BUF: usize = 4 * 1024;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Raise the soft fd limit to the hard cap — the real ceiling we're measuring.
    let _ = rlimit::increase_nofile_limit(u64::MAX);
    let fd_limit = rlimit::Resource::NOFILE
        .get()
        .map(|(soft, _)| soft)
        .unwrap_or(256) as usize;

    // Loopback puts both ends in this one process, so a connection costs 2 fds.
    // That fd cap is the system ceiling; default the target to 80% of it.
    let max_conns = fd_limit / 2;
    let target: usize = arg(1).unwrap_or(max_conns * 4 / 5);
    let client_tasks: usize = arg(2).unwrap_or(256);
    let ports = PORTS;
    println!(
        "Connection scale: target {target} held connections (80% of the ~{max_conns} fd ceiling)\n\
         fd limit {fd_limit} (2 fds/connection, both ends in-process), {ports} ports, {client_tasks} client tasks\n"
    );

    // Server: one acceptor process per port; each accepted connection becomes its own
    // process that just parks on the socket (holding it open).
    let runtime = Runtime::new();
    let accepted = Arc::new(AtomicU64::new(0));
    let mut addrs = Vec::with_capacity(ports);
    for _ in 0..ports {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        addrs.push(listener.local_addr()?);
        let accept_rt = runtime.clone();
        let count = Arc::clone(&accepted);
        runtime.spawn(move |_ctx| async move {
            while let Ok((stream, _)) = listener.accept().await {
                shrink(&stream);
                count.fetch_add(1, Ordering::Relaxed);
                accept_rt.spawn(move |_ctx| async move {
                    let mut stream = stream;
                    let mut buf = [0u8; 1];
                    // Park until the client closes; one held connection = one process.
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                });
            }
        });
    }

    // Clients: ramp connections round-robin across ports, holding each open, until we
    // reach the target or connects start failing (the fd wall).
    let addrs = Arc::new(addrs);
    let stop = Arc::new(AtomicBool::new(false));
    let opened = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    let tasks: Vec<_> = (0..client_tasks)
        .map(|t| {
            let (addrs, stop, opened, failed) =
                (addrs.clone(), stop.clone(), opened.clone(), failed.clone());
            tokio::spawn(async move {
                let mut held: Vec<TcpStream> = Vec::new();
                // This task owns source ports SRC_BASE+t, +t+client_tasks, … (a stripe
                // disjoint from every other task), each paired with all destinations —
                // so no two tasks ever race to bind the same port. The k-th connection:
                let ports = addrs.len();
                let mut k = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    if opened.load(Ordering::Relaxed) as usize >= target {
                        break;
                    }
                    let dst = addrs[k % ports];
                    let src_port = SRC_BASE as usize + t + (k / ports) * client_tasks;
                    k += 1;
                    if src_port >= 49_000 {
                        break; // ran out of source-port space below the ephemeral range
                    }
                    match connect(src_port as u16, dst).await {
                        Ok(stream) => {
                            held.push(stream);
                            opened.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            // At the fd wall, back off briefly rather than spin.
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    }
                }
                // Hold the connections open until told to stop.
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                held.len()
            })
        })
        .collect();

    // Progress: report held connections until the ramp settles (target reached, or
    // failures pile up at the ceiling).
    let mut last = 0u64;
    let mut stalls = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = accepted.load(Ordering::Relaxed);
        let fails = failed.load(Ordering::Relaxed);
        println!(
            "  held {now:>8}   (+{:>6}/0.5s)   connect failures: {fails}",
            now - last
        );
        if now as usize >= target {
            println!("\nreached target.");
            break;
        }
        // A plateau is the ceiling, whatever the cause — fd EMFILE (failures climb) or
        // the source-port space running out (tasks stop cleanly, no failures). Exit on
        // no-growth either way, so a too-high target can never hang the reporter.
        if now == last && now > 0 {
            stalls += 1;
            if stalls >= 3 {
                println!("\nramp plateaued — this is the ceiling ({fails} connect failures).");
                break;
            }
        } else {
            stalls = 0;
        }
        last = now;
    }

    let peak = accepted.load(Ordering::Relaxed);
    let elapsed = start.elapsed().as_secs_f64();
    let rss_kb = rss_kb();

    stop.store(true, Ordering::Relaxed);
    let mut client_held = 0usize;
    for task in tasks {
        client_held += task.await.unwrap_or(0);
    }

    println!("\n── peak ──");
    println!("concurrent connections held: {peak}   (each its own rusm-otp process)");
    println!("client-side sockets held:    {client_held}");
    println!(
        "ramp:                        {:.0} connections/sec over {elapsed:.1}s",
        peak as f64 / elapsed
    );
    if let Some(kb) = rss_kb {
        println!(
            "process RSS at peak:         {:.1} GB   ({} KB/connection)",
            kb as f64 / 1024.0 / 1024.0,
            if peak > 0 { kb / peak } else { 0 }
        );
    }
    println!(
        "fd ceiling:                  {fd_limit} fds ≈ {} connections (2 fds each)",
        fd_limit / 2
    );
    Ok(())
}

/// Connect from an explicit source port (bound with `SO_REUSEADDR` so many
/// connections share source ports across destinations — the 4-tuple stays unique,
/// dodging ephemeral-port exhaustion), with small buffers and RST-on-close.
async fn connect(src_port: u16, addr: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    socket.set_reuse_address(true).ok();
    socket.set_send_buffer_size(SOCK_BUF).ok();
    socket.set_recv_buffer_size(SOCK_BUF).ok();
    socket.set_linger(Some(Duration::ZERO)).ok();
    let src: SocketAddr = (std::net::Ipv4Addr::LOCALHOST, src_port).into();
    socket.bind(&src.into())?;
    socket.set_nonblocking(true)?;
    match socket.connect(&addr.into()) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }
    let stream = TcpStream::from_std(std::net::TcpStream::from(socket))?;
    // Wait until writable (connection established), then surface any connect error.
    stream.writable().await?;
    if let Some(err) = stream.take_error()? {
        return Err(err);
    }
    Ok(stream)
}

fn shrink(stream: &TcpStream) {
    let sock = socket2::SockRef::from(stream);
    sock.set_send_buffer_size(SOCK_BUF).ok();
    sock.set_recv_buffer_size(SOCK_BUF).ok();
}

/// Resident set size of this process in KB (via `ps`), or `None` if unavailable.
fn rss_kb() -> Option<u64> {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn arg<T: std::str::FromStr>(n: usize) -> Option<T> {
    std::env::args().nth(n).and_then(|s| s.parse().ok())
}
