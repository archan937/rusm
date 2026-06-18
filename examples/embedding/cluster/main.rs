//! A tiny **two-node RUSM cluster** — the smallest thing that shows processes
//! messaging each other across nodes.
//!
//! Run it:
//! ```sh
//! cargo run -p rusm-bench --example cluster
//! ```
//!
//! What it demonstrates, in order:
//!   1. a shared cluster **identity** (the TLS certificate every node presents),
//!   2. **binding** two nodes, each with its own local runtime,
//!   3. a **cluster-wide registered name** (`register_global`),
//!   4. **connecting** one node to the other,
//!   5. sending to a process **by global name** (`send_global`) — the sender never
//!      needs to know which node the process lives on,
//!   6. **live attach**: asking a remote node what it's running (`remote_pids`).

use std::net::SocketAddr;
use std::time::Duration;

use rusm_cluster::{ClusterNode, Identity};
use rusm_otp::Runtime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local: SocketAddr = "127.0.0.1:0".parse()?; // OS picks a free port

    // 1. One identity for the whole cluster. A node only completes a handshake with
    //    a peer presenting this same certificate; anyone else is rejected by TLS.
    let id = Identity::generate()?;

    // 2 + 3. Node "london" runs a greeter process and publishes it cluster-wide as
    //        "greeter". `register_global` registers it locally *and* gossips the
    //        name to every connected peer.
    let london = ClusterNode::bind("london", Runtime::new(), local, &id)?;
    let greeter = london.runtime().spawn(|mut ctx| async move {
        while let Some(msg) = ctx.recv().await.message() {
            println!(
                "   [london] greeter received: {}",
                String::from_utf8_lossy(&msg)
            );
        }
    });
    london.register_global("greeter", greeter.pid());

    // 4. Node "tokyo" dials london. The handshake teaches each side the other's name.
    let tokyo = ClusterNode::bind("tokyo", Runtime::new(), local, &id)?;
    tokyo.connect(london.local_addr()?).await?;
    println!("[tokyo] connected to: {:?}", tokyo.peers());

    // 5. Wait for the "greeter" registration to gossip over, then send to it BY NAME.
    //    tokyo never learns *where* greeter is — the cluster resolves it.
    while tokyo.whereis_global("greeter").is_none() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    println!(
        "[tokyo] 'greeter' lives on node: {:?}",
        tokyo.whereis_global("greeter").unwrap()
    );
    tokyo.send_global("greeter", b"hello from tokyo!").await?;

    // 6. Live attach: see what london is running, from tokyo.
    let pids = tokyo.remote_pids("london").await?;
    println!("[tokyo] london is running {} process(es)", pids.len());

    // Give the cross-node message a moment to print before we exit.
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}
