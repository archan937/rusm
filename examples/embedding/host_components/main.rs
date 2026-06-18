//! **Hosting WASM components on RUSM's actor model** (Phase 7).
//!
//! Shows the core of what RUSM does: load a real WASM *component*, run it as an
//! isolated, observable process, and sandbox it with a capability profile — the
//! component-model artifact of wasmCloud, on the BEAM's process model.
//!
//! Run: `cargo run -p rusm-bench --example host_components`

use rusm_otp::{ExitReason, ProcessHandle, Received, Runtime};
use rusm_wasm::{Capabilities, WasmRuntime};

// A minimal component (component-model `.wat`, accepted directly): one page of
// linear memory and a `run` export that returns immediately.
const HELLO: &str = r#"(component
    (core module $m (memory (export "mem") 1) (func (export "run")))
    (core instance $i (instantiate $m))
    (func (export "run") (canon lift (core func $i "run"))))"#;

// A "hungry" component: tries to grow memory by two pages and traps if the growth
// is denied — so a tight memory cap makes it crash, a generous one lets it finish.
const HUNGRY: &str = r#"(component
    (core module $m
        (memory (export "mem") 1)
        (func (export "run")
            (if (i32.eq (memory.grow (i32.const 2)) (i32.const -1)) (then unreachable))))
    (core instance $i (instantiate $m))
    (func (export "run") (canon lift (core func $i "run"))))"#;

/// Spawns `guest` and reports the exit reason a monitor observes.
async fn outcome(rt: &Runtime, guest: ProcessHandle) -> ExitReason {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let watcher = rt
        .spawn(move |mut ctx| async move {
            if let Received::Down { reason, .. } = ctx.recv().await {
                let _ = tx.send(reason);
            }
        })
        .pid();
    rt.monitor(watcher, guest.pid());
    rx.await.unwrap()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    // `with_mailbox_depth` turns on the (otherwise off) depth counter so `info`
    // reports it — what an observer/REPL node would do.
    let rt = Runtime::with_mailbox_depth();
    let wasm = WasmRuntime::new(rt.clone()).expect("wasm engine");

    // 1) Host a component as a process. `prepare` resolves imports + the entry
    //    export once; each spawn is a fresh, isolated instance.
    let hello = wasm
        .prepare_component(&wasm.compile_component(HELLO).unwrap(), "run")
        .unwrap();
    let p = wasm.spawn_component(&hello);
    let pid = p.pid();
    println!("hosted a component as process {pid:?}");
    if let Some(info) = rt.info(pid) {
        println!(
            "  Process.info -> links={}, mailbox_depth={}",
            info.links, info.mailbox_depth
        );
    }
    println!("  Process.list -> {:?}", rt.list());
    p.join().await;
    println!(
        "  it ran and was reaped; live processes now: {}\n",
        rt.process_count()
    );

    // 2) Capabilities (default-deny). A tight memory cap denies the hungry
    //    component's growth -> it traps -> Crashed. A generous cap lets it finish.
    let hungry = wasm
        .prepare_component(&wasm.compile_component(HUNGRY).unwrap(), "run")
        .unwrap();

    let capped = wasm.spawn_component_with(&hungry, Capabilities::nothing().max_memory(64 << 10));
    println!(
        "hungry component, 64 KiB cap  -> {:?}",
        outcome(&rt, capped).await
    );

    let roomy = wasm.spawn_component_with(&hungry, Capabilities::nothing().max_memory(8 << 20));
    println!(
        "hungry component, 8 MiB cap   -> {:?}",
        outcome(&rt, roomy).await
    );

    println!("\nSame component, two capability profiles — sandboxed by construction.");
}
