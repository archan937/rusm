//! The **wasip2 bridge**: run a WASI **component** as a `rusm-otp` process.
//!
//! Mirrors the core-module path in [`crate`] (`compile`/`prepare`/`spawn`) but for
//! the component model — `Component`, a component [`Linker`](ComponentLinker) with
//! WASI p2 wired in, and a per-process [`WasiHost`]. Instance-per-process, epoch
//! preemption and the pooling allocator all carry over unchanged; a trap exits the
//! process [`Crashed`](ExitReason::Crashed). The shared efficiency levers live in
//! `lib.rs`; this file is only the component-specific glue.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use rusm_otp::{Context, ExitReason, ProcessHandle};
use wasmtime::component::{
    Component, ComponentExportIndex, InstancePre as ComponentInstancePre, Linker as ComponentLinker,
};
use wasmtime::{Engine, ResourceLimiter, Store};
use wasmtime_wasi::p2::bindings::CommandPre;
use wasmtime_wasi::{ResourceTable, WasiCtx};

use super::{HttpCaps, WasiHost};
use crate::bridges::serve::ConnectionInfo;
use crate::caps::{Capabilities, CapabilityProfile};
use crate::{Spawner, WasmRuntime};

/// A component whose imports are resolved **and** whose entry-export index is
/// precomputed — so a spawn skips both per-spawn import resolution *and* the
/// by-name export lookup. Opaque on purpose: it hides the internal WASI host type.
#[derive(Clone)]
pub struct PreparedComponent {
    pre: ComponentInstancePre<WasiHost>,
    /// The `entry` export resolved once at prepare time (index, not a string).
    entry: ComponentExportIndex,
    /// The same component prepared against the **overflow** engine (its own
    /// `InstancePre` + entry index). `None` unless the runtime has an overflow tier
    /// ([`WasmRuntime::with_overflow`]); used when the pooled tier is full.
    overflow: Option<(ComponentInstancePre<WasiHost>, ComponentExportIndex)>,
}

/// Builds the component linker once, with WASI **p2 and p3** wired in plus the
/// `rusm:runtime` actor ABI — all sharing one [`WasiHost`]. A component importing
/// the `@0.2.0` or `@0.3.0` WASI interfaces resolves against the same host.
pub(crate) fn build_linker(engine: &Engine) -> Result<ComponentLinker<WasiHost>> {
    let mut linker = ComponentLinker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    super::wasip3::add_to_linker(&mut linker)?;
    // wasi:http imports, so a component can be served as an HTTP handler (Phase 11)
    // and outbound `wasi:http` resolves. Idle for non-HTTP guests.
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    crate::bridges::actor::add_to_linker(&mut linker)?;
    super::kv::add_to_linker(&mut linker)?;
    super::log::add_to_linker(&mut linker)?;
    super::pg::add_to_linker(&mut linker)?;
    super::serve::add_to_linker(&mut linker)?;
    super::streams::add_to_linker(&mut linker)?;
    Ok(linker)
}

/// Resolve a component's imports against `linker` (and, if present, the overflow tier) and
/// precompute its `entry` export index — the expensive `instantiate_pre`, done once, never per
/// spawn. Shared by [`WasmRuntime::prepare_component`] and the dynamic-WASM preparer (which has
/// the engine + linker but not a `&WasmRuntime`), so the prepare logic lives in one place.
pub(crate) fn prepare_component_with(
    linker: &ComponentLinker<WasiHost>,
    overflow: Option<(&Engine, &ComponentLinker<WasiHost>)>,
    component: &Component,
    entry: &str,
) -> Result<PreparedComponent> {
    let pre = linker.instantiate_pre(component)?;
    let entry_index = component
        .get_export_index(None, entry)
        .ok_or_else(|| anyhow::anyhow!("component has no `{entry}` export"))?;
    // With an overflow tier, prepare the same component against it too — without recompiling:
    // serialize the already-compiled component and load it into the overflow engine.
    let overflow = match overflow {
        Some((engine, overflow_linker)) => {
            let cwasm = component.serialize()?;
            // Safety: `cwasm` was just produced by `serialize` on a trusted, in-process
            // component — exactly the precondition `deserialize` wants.
            let overflow_component = unsafe { Component::deserialize(engine, &cwasm)? };
            let overflow_pre = overflow_linker.instantiate_pre(&overflow_component)?;
            let overflow_entry = overflow_component
                .get_export_index(None, entry)
                .ok_or_else(|| anyhow::anyhow!("component has no `{entry}` export"))?;
            Some((overflow_pre, overflow_entry))
        }
        None => None,
    };
    Ok(PreparedComponent {
        pre,
        entry: entry_index,
        overflow,
    })
}

impl WasmRuntime {
    /// Compiles a component from Wasm bytes or component-model `.wat` text.
    pub fn compile_component(&self, wasm: impl AsRef<[u8]>) -> Result<Component> {
        Ok(Component::new(&self.spawner.engine, wasm)?)
    }

    /// Resolves a component's imports **once** against the WASI linker and
    /// precomputes its `entry` export index — the fast path for spawning the same
    /// component+entrypoint many times (no per-spawn import resolution or by-name
    /// export lookup). Errors if the component has no such export.
    pub fn prepare_component(
        &self,
        component: &Component,
        entry: &str,
    ) -> Result<PreparedComponent> {
        let overflow = match (&self.spawner.overflow, &self.overflow_component_linker) {
            (Some(engine), Some(linker)) => Some((engine, linker.as_ref())),
            _ => None,
        };
        prepare_component_with(&self.component_linker, overflow, component, entry)
    }

    /// Prepare a component from a **dynamic source** through the content-addressed compile
    /// cache — the embedding-facing twin of a guest's `spawn-from`. The first call for a
    /// given bundle fetches + compiles + prepares it (cold); every later call for the same
    /// bytes returns the cached [`PreparedComponent`] (hot), single-flighted across
    /// concurrent callers. Pair with [`spawn_component`](Self::spawn_component). `source` is
    /// `inline:<bytes>`, `kv:<bucket>/<key>` (needs a store), or `url:`/`http(s)://…` (needs
    /// a resolver set via [`set_bundle_resolver`](Self::set_bundle_resolver)). Capability
    /// gating is the host's concern here — there is no guest profile to enforce, exactly as
    /// for [`spawn_component`](Self::spawn_component).
    pub async fn prepare_dynamic(&self, source: &str) -> Result<Arc<PreparedComponent>, String> {
        self.spawner.dynamic_wasm(source).await
    }

    /// Spawns a prepared component as an isolated process under the **default-deny
    /// `Sandboxed`** profile (no fs/net/env, a bounded heap). Use
    /// [`spawn_component_with`](WasmRuntime::spawn_component_with) to grant more.
    pub fn spawn_component(&self, prepared: &PreparedComponent) -> ProcessHandle {
        self.spawn_component_with(prepared, CapabilityProfile::Sandboxed.capabilities())
    }

    /// Spawns a prepared component as an isolated process running its entry export,
    /// under the given [`Capabilities`]. A fresh instance + WASI context per
    /// process; a trap (or a denied capability the guest turns into a trap) exits
    /// the process [`Crashed`](ExitReason::Crashed).
    pub fn spawn_component_with(
        &self,
        prepared: &PreparedComponent,
        caps: Capabilities,
    ) -> ProcessHandle {
        // Unnamed: the platform log names only components spawned *by name* (see the
        // named sites / `note_spawn`); this raw entry stays out of the lifecycle log.
        self.spawner.spawn_component(prepared, caps, None)
    }

    /// Spawns a **stock command component** (one exporting `wasi:cli/run` — any
    /// language's wasip2 binary, no `rusm:runtime` required) as an isolated process
    /// under the default-deny `Sandboxed` profile. It runs to completion, then exits.
    pub fn spawn_command(&self, component: &Component) -> Result<ProcessHandle> {
        self.spawn_command_with(component, CapabilityProfile::Sandboxed.capabilities())
    }

    /// Like [`spawn_command`](Self::spawn_command) but under explicit [`Capabilities`].
    /// Errors if `component` doesn't satisfy the `wasi:cli` command world.
    pub fn spawn_command_with(
        &self,
        component: &Component,
        caps: Capabilities,
    ) -> Result<ProcessHandle> {
        let pre = CommandPre::new(self.component_linker.instantiate_pre(component)?)?;
        let spawner = Arc::clone(&self.spawner);
        Ok(self
            .spawner
            .rt
            .spawn(move |ctx| run_command(spawner, pre, caps, ctx)))
    }
}

impl Spawner {
    /// Spawns a prepared component as an isolated process under `caps` — the single
    /// spawn path shared by the public [`spawn_component_with`] and a guest's
    /// capability-gated `spawn`. The child carries an `Arc` to this spawner, so it
    /// too can spawn siblings by name.
    ///
    /// [`spawn_component_with`]: WasmRuntime::spawn_component_with
    pub(crate) fn spawn_component(
        self: &Arc<Self>,
        prepared: &PreparedComponent,
        caps: Capabilities,
        label: Option<&str>,
    ) -> ProcessHandle {
        // Capture what the platform log needs *before* `caps` moves into the body, and
        // only when a named spawn is actually being logged — so the off path (and every
        // unnamed/internal spawn) stays allocation-free on the hot path.
        let log_caps = label
            .filter(|_| self.rt.wants_log(rusm_otp::LogLevel::Error))
            .map(|_| caps.clone());
        let spawner = Arc::clone(self);
        let prepared = prepared.clone();
        let handle = self
            .rt
            .spawn(move |ctx| run(spawner, prepared, caps, ctx, None, None, None));
        if let (Some(label), Some(caps)) = (label, &log_caps) {
            self.record_spawn(handle.pid(), label, caps);
        }
        handle
    }

    /// Spawns a prepared component as a **per-connection serving handler** (WebSocket or
    /// SSE), carrying the connection's [`ConnectionInfo`] into its store so the guest can
    /// read it via the `connection` actor op. The serving twin of [`spawn_component`]; the
    /// handshake (writer pid as message 1) is unchanged, so this is purely additive — the
    /// context rides in per-instance state, not the mailbox.
    pub(crate) fn spawn_connection(
        self: &Arc<Self>,
        prepared: &PreparedComponent,
        caps: Capabilities,
        connection: ConnectionInfo,
        ws_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::WsOut>>,
        sse_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::SseEvent>>,
    ) -> ProcessHandle {
        let spawner = Arc::clone(self);
        let prepared = prepared.clone();
        self.rt.spawn(move |ctx| {
            run(
                spawner,
                prepared,
                caps,
                ctx,
                Some(connection),
                ws_out,
                sse_out,
            )
        })
    }

    /// Spawns a component registered by `name` under its **declared** profile — the
    /// host twin of the guest `spawn` ABI (`actor::spawn`), without a guest's
    /// caps-or-spawn gating (the node operator is the caller). `None` if nothing is
    /// registered under `name`. Feeds a TS service its bundle as message 1, exactly
    /// like the guest path. Re-runnable, so it backs a supervised resident's restart.
    pub(crate) fn spawn_registered(self: &Arc<Self>, name: &str) -> Option<ProcessHandle> {
        let entry = self.lookup(name)?;
        let caps = entry
            .caps
            .clone()
            .unwrap_or_else(|| CapabilityProfile::Sandboxed.capabilities());
        // A boot/resident component is fixed (never a dynamic template), so `prepared` is
        // present; `None` (a template) has nothing to boot, so there's nothing to spawn here.
        let handle = self.spawn_component(entry.prepared.as_ref()?, caps, Some(name));
        if let Some(bundle) = &entry.bundle {
            self.rt.send(handle.pid(), (**bundle).clone());
        }
        Some(handle)
    }

    /// The single platform-log policy for a **named** spawn: label the process (so its
    /// later `exit` line can name it, even below `Debug`) and, at `Debug`, emit the
    /// `spawn` line carrying its effective capabilities. Every named spawn site funnels
    /// here; call only when [`Runtime::wants_log`]`(Error)` (the caller has gated it).
    pub(crate) fn record_spawn(&self, pid: rusm_otp::Pid, label: &str, caps: &Capabilities) {
        self.rt.set_label(pid, label);
        if self.rt.wants_log(rusm_otp::LogLevel::Debug) {
            self.rt.log_spawn(pid, label, &caps.summary());
        }
    }
}

/// Decrements the pooled-tier live count when a pooled instance's process ends —
/// keeping `pooled_live` an exact mirror of occupied pool slots, on every exit path
/// (completion, trap, or kill, since this drops as the process body unwinds).
struct PoolSlot(Arc<Spawner>);
impl Drop for PoolSlot {
    fn drop(&mut self) {
        self.0.pooled_live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Pick the engine + prepared instance for this spawn, **consuming** `prepared` so
/// the chosen `InstancePre` is moved (not re-cloned) onto the hot path. With an
/// overflow tier, a spawn **reserves a pooled slot** if one is free (returning a
/// [`PoolSlot`] guard); once the pool is full it falls to the overflow engine.
/// Without a tier (the default), it's always the pooled fast path and reserves
/// nothing — byte-for-byte the pre-overflow behaviour.
fn select_tier(
    spawner: &Arc<Spawner>,
    prepared: PreparedComponent,
) -> (
    Engine,
    ComponentInstancePre<WasiHost>,
    ComponentExportIndex,
    Option<PoolSlot>,
) {
    match (&spawner.overflow, prepared.overflow) {
        (Some(overflow_engine), Some((overflow_pre, overflow_entry))) => {
            // Claim a pooled slot iff the prior count was below capacity. `cap`
            // claims can be outstanding at once — exactly the pool's slot count —
            // so a claimed pooled spawn never hits pool exhaustion.
            if spawner.pooled_live.fetch_add(1, Ordering::AcqRel) < spawner.pooled_cap {
                let slot = PoolSlot(Arc::clone(spawner));
                (
                    spawner.engine.clone(),
                    prepared.pre,
                    prepared.entry,
                    Some(slot),
                )
            } else {
                spawner.pooled_live.fetch_sub(1, Ordering::AcqRel);
                (overflow_engine.clone(), overflow_pre, overflow_entry, None)
            }
        }
        _ => (spawner.engine.clone(), prepared.pre, prepared.entry, None),
    }
}

/// The process body for a component: build its WASI context, instantiate it in a
/// fresh store, and run its entry export — exiting [`Crashed`](ExitReason::Crashed)
/// on any failure. The runtime handle is cloned exactly once (into the host) and
/// the crash-exit reads it back through the store; the engine is borrowed from the
/// `Arc<Spawner>` the host carries. Yields to the scheduler on each epoch tick.
/// Builds the per-process store + [`WasiHost`] shared by the actor and command paths:
/// the WASI context, memory limiter, and epoch deadline. The runtime handle is cloned
/// exactly once (into the host); the crash-exit path reads it back through the store.
fn build_store(
    spawner: Arc<Spawner>,
    engine: &Engine,
    wasi: WasiCtx,
    caps: Capabilities,
    ctx: Context,
    connection: Option<ConnectionInfo>,
    ws_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::WsOut>>,
    sse_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::SseEvent>>,
) -> Store<WasiHost> {
    let host = WasiHost {
        wasi,
        table: ResourceTable::new(),
        http: wasmtime_wasi_http::WasiHttpCtx::new(),
        http_hooks: HttpCaps {
            allow_network: caps.network_allowed(),
        },
        pid: ctx.pid().raw(),
        connection,
        ws_out,
        sse_out,
        caps,
        rt: spawner.rt.clone(),
        ctx: Some(ctx),
        spawner: Some(spawner),
        out_streams: HashMap::new(),
        in_streams: HashMap::new(),
        next_stream: 0,
        timers: HashMap::new(),
        next_timer: 0,
    };
    let mut store = Store::new(engine, host);
    // Enforce the per-process memory ceiling (WasiHost is the ResourceLimiter).
    store.limiter(|host| host as &mut dyn ResourceLimiter);
    store.set_epoch_deadline(1);
    store.epoch_deadline_async_yield_and_update(1);
    store
}

async fn run(
    spawner: Arc<Spawner>,
    prepared: PreparedComponent,
    caps: Capabilities,
    ctx: Context,
    connection: Option<ConnectionInfo>,
    ws_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::WsOut>>,
    sse_out: Option<tokio::sync::mpsc::Sender<crate::bridges::conn::SseEvent>>,
) {
    let pid = ctx.pid();
    let wasi = match caps.build_wasi() {
        Ok(wasi) => wasi,
        Err(_) => {
            spawner.rt.exit(pid, ExitReason::Crashed);
            return;
        }
    };
    // Choose the pooled (fast) tier or the on-demand overflow tier. `_slot` holds a
    // pooled reservation for this process's lifetime (dropped when `run` returns).
    let (engine, pre, entry, _slot) = select_tier(&spawner, prepared);
    let mut store = build_store(
        spawner, &engine, wasi, caps, ctx, connection, ws_out, sse_out,
    );

    let outcome = async {
        let instance = pre.instantiate_async(&mut store).await?;
        // Precomputed index — no per-spawn by-name export lookup.
        let func = instance.get_typed_func::<(), ()>(&mut store, entry)?;
        func.call_async(&mut store, ()).await
    }
    .await;
    if outcome.is_err() {
        // The host (and its runtime handle) is still in the store — no extra clone.
        store.data().rt.exit(pid, ExitReason::Crashed);
    }
}

/// The process body for a **stock command component** (`wasi:cli/run`): instantiate the
/// `Command` world and run it to completion. A host trap *or* a non-zero program exit
/// leaves the process [`Crashed`](ExitReason::Crashed); a clean run exits normally.
/// Commands use the pooled engine (no overflow tier — they're one-shot, not long-lived).
async fn run_command(
    spawner: Arc<Spawner>,
    pre: CommandPre<WasiHost>,
    caps: Capabilities,
    ctx: Context,
) {
    let pid = ctx.pid();
    let wasi = match caps.build_wasi() {
        Ok(wasi) => wasi,
        Err(_) => {
            spawner.rt.exit(pid, ExitReason::Crashed);
            return;
        }
    };
    let engine = spawner.engine.clone();
    let mut store = build_store(spawner, &engine, wasi, caps, ctx, None, None, None);

    let outcome = async {
        let command = pre.instantiate_async(&mut store).await?;
        command
            .wasi_cli_run()
            .call_run(&mut store)
            .await?
            .map_err(|()| anyhow::anyhow!("command component exited with a failure status"))
    }
    .await;
    if outcome.is_err() {
        store.data().rt.exit(pid, ExitReason::Crashed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusm_otp::Runtime;

    // A minimal component exporting `run: func()` — no imports, no WASI use.
    const COMP_RUN: &str = r#"(component
        (core module $m (func (export "run")))
        (core instance $i (instantiate $m))
        (func (export "run") (canon lift (core func $i "run"))))"#;

    const COMP_TRAP: &str = r#"(component
        (core module $m (func (export "run") unreachable))
        (core instance $i (instantiate $m))
        (func (export "run") (canon lift (core func $i "run"))))"#;

    const COMP_SPIN: &str = r#"(component
        (core module $m (func (export "run") (loop (br 0))))
        (core instance $i (instantiate $m))
        (func (export "run") (canon lift (core func $i "run"))))"#;

    // Starts with one page (64 KiB) and tries to grow by two more (to 192 KiB);
    // if growth is denied (memory.grow returns -1) it traps. So a memory cap below
    // 192 KiB makes it crash, a cap at/above it lets it finish normally.
    const COMP_GROW: &str = r#"(component
        (core module $m
            (memory (export "mem") 1)
            (func (export "run")
                (if (i32.eq (memory.grow (i32.const 2)) (i32.const -1))
                    (then unreachable))))
        (core instance $i (instantiate $m))
        (func (export "run") (canon lift (core func $i "run"))))"#;

    /// Exits Normal iff its declared env grant `RUSM_CAP_PROBE="granted"` reached it,
    /// else panics (trap → Crashed). See `tests/fixtures/env-gate`.
    const ENV_GATE: &[u8] = include_bytes!("../../tests/fixtures/env_gate.wasm");

    /// Monitors a freshly spawned guest and returns the exit reason it observes.
    async fn exit_reason_of(rt: &Runtime, guest: &ProcessHandle) -> ExitReason {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let watcher = rt
            .spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await);
            })
            .pid();
        rt.monitor(watcher, guest.pid());
        match rx.await.unwrap() {
            rusm_otp::Received::Down { reason, .. } => reason,
            other => panic!("expected a Down, got {other:?}"),
        }
    }

    /// Monitors a process *by pid* (a guest-spawned child returns only its pid).
    async fn exit_reason_of_pid(rt: &Runtime, pid: rusm_otp::Pid) -> ExitReason {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let watcher = rt
            .spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await);
            })
            .pid();
        rt.monitor(watcher, pid);
        match rx.await.unwrap() {
            rusm_otp::Received::Down { reason, .. } => reason,
            other => panic!("expected a Down, got {other:?}"),
        }
    }

    /// A bare [`WasiHost`] wired to the runtime's spawner — stands in for a running
    /// guest so the capability-gated `spawn` host fn can be driven directly.
    fn test_host(wr: &WasmRuntime, rt: &Runtime, caps: Capabilities) -> WasiHost {
        WasiHost {
            wasi: caps.build_wasi().unwrap(),
            table: ResourceTable::new(),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
            http_hooks: HttpCaps {
                allow_network: caps.network_allowed(),
            },
            pid: 0,
            connection: None,
            ws_out: None,
            sse_out: None,
            caps,
            rt: rt.clone(),
            ctx: None,
            spawner: Some(Arc::clone(&wr.spawner)),
            out_streams: HashMap::new(),
            in_streams: HashMap::new(),
            next_stream: 0,
            timers: HashMap::new(),
            next_timer: 0,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_can_spawn_a_registered_component() {
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let child = wr
            .prepare_component(&wr.compile_component(COMP_RUN).unwrap(), "run")
            .unwrap();
        wr.register_component("child", child);

        // Trusted grants the spawn capability.
        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        host.spawn("child".to_string())
            .await
            .expect("spawn of a registered component succeeds");

        // The child runs to completion as a real, reaped process.
        for _ in 0..200 {
            if rt.finished() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(rt.finished(), 1, "the spawned child ran and was reaped");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_registered_component_runs_under_its_declared_profile_not_the_spawners() {
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let gate = wr
            .prepare_component(&wr.compile_component(ENV_GATE).unwrap(), "run")
            .unwrap();
        // Registered WITH a declared profile that grants the env var the component needs.
        wr.register_component_with(
            "gate",
            gate,
            CapabilityProfile::Trusted
                .capabilities()
                .env("RUSM_CAP_PROBE", "granted"),
        );

        // The spawner has `spawn` (Trusted) but NOT the env grant. If the child inherited
        // the spawner's caps (the old behavior) it would see no env and panic; the
        // component's own declared profile must win instead.
        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        host.spawn("gate".to_string())
            .await
            .expect("spawn of a registered component succeeds");

        for _ in 0..200 {
            if rt.finished() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            rt.finished(),
            1,
            "the child must run under its declared profile (env granted), not the spawner's"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn overflow_tier_spawns_past_the_pooled_cap() {
        let rt = Runtime::new();
        // A pool of 2, with an on-demand overflow tier for the rest.
        let wr = WasmRuntime::with_overflow(rt.clone(), 2, crate::DEFAULT_MAX_MEMORY).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(COMP_SPIN).unwrap(), "run")
            .unwrap();

        // Five long-lived (epoch-preempted infinite-loop) instances — more than the
        // pool holds. With overflow, all five come alive; the 3 past the pool ran on
        // the on-demand engine.
        let handles: Vec<_> = (0..5).map(|_| wr.spawn_component(&pre)).collect();
        for _ in 0..400 {
            if rt.process_count() == 5 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            rt.process_count(),
            5,
            "overflow caught the instances past the pool of 2"
        );
        for h in handles {
            h.kill();
            h.join().await; // await teardown so no spinner outlives the test
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_is_denied_without_the_capability() {
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let child = wr
            .prepare_component(&wr.compile_component(COMP_RUN).unwrap(), "run")
            .unwrap();
        wr.register_component("child", child);

        // Sandboxed (default-deny): no spawn capability.
        let mut host = test_host(&wr, &rt, CapabilityProfile::Sandboxed.capabilities());
        let err = host.spawn("child".to_string()).await.unwrap_err();
        assert!(
            err.contains("denied"),
            "sandboxed spawn must be denied: {err}"
        );
        assert_eq!(rt.process_count(), 0, "no child was created");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_of_an_unknown_component_errors() {
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        let err = host.spawn("ghost".to_string()).await.unwrap_err();
        assert!(err.contains("unknown component"), "{err}");
        assert_eq!(rt.process_count(), 0, "nothing spawned");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_spawned_child_inherits_the_parents_capabilities() {
        use crate::bindings::rusm::runtime::actor::Host;
        // The child is non-escalating: it gets the spawner's caps. A parent with a
        // tight memory cap yields a child that crashes growing; a roomy parent's
        // child finishes — same component, capability inherited from the spawner.
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let grow = wr
            .prepare_component(&wr.compile_component(COMP_GROW).unwrap(), "run")
            .unwrap();
        wr.register_component("grow", grow);

        let tight = Capabilities::nothing()
            .allow_spawn(true)
            .max_memory(64 << 10);
        let mut parent = test_host(&wr, &rt, tight);
        let crashed = parent.spawn("grow".to_string()).await.unwrap();
        assert_eq!(
            exit_reason_of_pid(&rt, rusm_otp::Pid::from_raw(crashed)).await,
            ExitReason::Crashed,
            "child inherited the tight cap and crashed growing"
        );

        let roomy = Capabilities::nothing()
            .allow_spawn(true)
            .max_memory(8 << 20);
        let mut parent = test_host(&wr, &rt, roomy);
        let finished = parent.spawn("grow".to_string()).await.unwrap();
        assert_eq!(
            exit_reason_of_pid(&rt, rusm_otp::Pid::from_raw(finished)).await,
            ExitReason::Normal,
            "child inherited the roomy cap and finished"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn components_call_each_other_via_the_actor_abi() {
        // Two instances of one component: the first registers "responder" and
        // serves request/reply; the second finds it via whereis and calls it,
        // forwarding the reply to a native collector — component-to-component
        // "callbacks" with no new runtime API, just the actor ABI.
        const CALLBACK: &[u8] = include_bytes!("../../tests/fixtures/callback.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(CALLBACK).unwrap(), "run")
            .unwrap();

        // Native collector receives the caller's final result.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        assert!(rt.register("collector", collector.pid()));

        // Instance 1 → responder; wait until it has registered the name.
        let _responder = wr.spawn_component(&pre);
        for _ in 0..200 {
            if rt.whereis("responder").is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(rt.whereis("responder").is_some(), "responder must register");

        // Instance 2 → caller: calls the responder (21 -> doubled -> 42).
        let _caller = wr.spawn_component(&pre);
        assert_eq!(rx.await.unwrap(), vec![42]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_component_streams_bytes_to_another() {
        // Two instances of one component: a producer opens a byte stream to a
        // consumer and writes 3x "hello!" (18 bytes); the consumer accepts it,
        // reads to EOF, and reports the total — cross-process streaming through the
        // actor world, Tokio-backpressured.
        const PIPE: &[u8] = include_bytes!("../../tests/fixtures/stream_pipe.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(PIPE).unwrap(), "run")
            .unwrap();

        // Native collector receives the consumer's byte total.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        let consumer = wr.spawn_component(&pre);
        let producer = wr.spawn_component(&pre);
        // consumer: [role 1][collector pid] — accept, read, report to collector.
        let mut cmsg = vec![1u8];
        cmsg.extend_from_slice(&collector.pid().raw().to_le_bytes());
        rt.send(consumer.pid(), cmsg);
        // producer: [role 0][consumer pid] — open, write, close.
        let mut pmsg = vec![0u8];
        pmsg.extend_from_slice(&consumer.pid().raw().to_le_bytes());
        rt.send(producer.pid(), pmsg);

        let total = rx.await.unwrap();
        assert_eq!(
            u32::from_le_bytes(total[..4].try_into().unwrap()),
            18,
            "consumer should read all 3x6 = 18 streamed bytes through to EOF"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_drives_the_actor_api() {
        // The rusm-ts js-runner: a component embedding rquickjs that runs a JS
        // bundle bridged to the actor world. The bundle uses `Process.receive`,
        // `Process.setLabel`, `Process.self`, and `Process.send` — proving a TS/JS
        // guest is a first-class, sandboxed RUSM process.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();   // msg: who to answer
                Process.setLabel("ts-worker");
                Process.send(replyTo, "pong from " + Process.self());
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        // spawn_js feeds the bundle as the first message; then the reply-to pid.
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());

        let reply = String::from_utf8(rx.await.unwrap()).unwrap();
        assert_eq!(
            reply,
            format!("pong from {}", guest.pid().raw()),
            "JS ran inside the component and drove the actor API"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn console_log_tolerates_bigint_pids() {
        // Pids surface as bigint; `console.log(Process.self())` must not throw
        // (JSON.stringify can't serialise bigint). If `fmt` threw, the bundle would
        // trap before replying — so a reply proves console handled the pid.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                console.log("my pid is", Process.self(), undefined);
                Process.send(replyTo, "logged ok");
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(String::from_utf8(rx.await.unwrap()).unwrap(), "logged ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_has_web_api_polyfills() {
        // The runner installs Web API polyfills (webapi.js) before the bundle, so a
        // TS guest gets URL/TextEncoder/etc. transparently — no host support needed.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const u = new URL("https://example.io:8080/a?x=1");
                const n = new TextEncoder().encode("héllo").length;   // é = 2 bytes → 6
                Process.send(replyTo, u.hostname + "|" + u.port + "|" + n);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "example.io|8080|6",
            "URL + TextEncoder polyfills work inside the guest"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_can_receive_with_a_timeout() {
        // `Process.receive(ms)` is Erlang's `receive … after`: it resolves to null
        // on an idle timeout, and to the message when one arrives before the
        // deadline. Same handshake as the Rust `actor-timeout` fixture.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const timedOut = (await Process.receive(30)) === null;
                Process.send(replyTo, "armed");
                const got = (await Process.receiveText(5000)) === "ping";
                Process.send(replyTo, (timedOut ? "1" : "0") + (got ? "1" : "0"));
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        let guest_pid = guest.pid();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let driver_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            driver_rt.send(guest_pid, ctx.pid().raw().to_string().into_bytes());
            assert_eq!(ctx.recv().await.message().unwrap(), b"armed");
            driver_rt.send(guest_pid, b"ping".to_vec());
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "11",
            "Process.receive(ms) times out when idle and returns a message before the deadline"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_has_crypto() {
        // `crypto.getRandomValues` + `randomUUID` (backed by host wasi:random) work in
        // a *sandboxed* guest — the entropy ecosystem (uuid/nanoid, ai-sdk) depends on.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const u = crypto.randomUUID();
                const arr = crypto.getRandomValues(new Uint8Array(8));
                const v4 = u.length === 36 && u[14] === "4" && "89ab".includes(u[19]);
                Process.send(replyTo, `${v4}|${arr.length}|${u.split("-").length}`);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "true|8|5",
            "crypto.randomUUID is a v4 UUID and getRandomValues fills the array"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_has_subtle_crypto() {
        // crypto.subtle (native RustCrypto): SHA-256 against a known vector, an HMAC
        // sign→verify round-trip plus tamper-rejection, and an AES-GCM encrypt→decrypt
        // round-trip — all in a *sandboxed* guest (no capability needed).
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const enc = new TextEncoder();
                const hex = (b) => [...new Uint8Array(b)].map(x => x.toString(16).padStart(2,"0")).join("");
                let flags = 0;
                // SHA-256("abc") known-answer test.
                const d = await crypto.subtle.digest("SHA-256", enc.encode("abc"));
                if (hex(d) === "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad") flags |= 1;
                // HMAC-SHA256: sign, verify (ok), verify (tampered → false).
                const k = await crypto.subtle.importKey("raw", enc.encode("secret"), {name:"HMAC", hash:"SHA-256"}, false, ["sign","verify"]);
                const sig = await crypto.subtle.sign("HMAC", k, enc.encode("msg"));
                if (await crypto.subtle.verify("HMAC", k, sig, enc.encode("msg"))) flags |= 2;
                if (!(await crypto.subtle.verify("HMAC", k, sig, enc.encode("tampered")))) flags |= 4;
                // AES-GCM: generate key, encrypt, decrypt round-trip.
                const ak = await crypto.subtle.generateKey({name:"AES-GCM", length:256}, true, ["encrypt","decrypt"]);
                const iv = crypto.getRandomValues(new Uint8Array(12));
                const ct = await crypto.subtle.encrypt({name:"AES-GCM", iv}, ak, enc.encode("hello"));
                const pt = await crypto.subtle.decrypt({name:"AES-GCM", iv}, ak, ct);
                if (new TextDecoder().decode(pt) === "hello") flags |= 8;
                Process.send(replyTo, String(flags));
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "15",
            "subtle: SHA-256 vector + HMAC sign/verify/tamper + AES-GCM round-trip"
        );
    }

    /// Query the pubsub broker for `topic`'s subscriber count.
    async fn pubsub_count(rt: &Runtime, broker: rusm_otp::Pid, topic: &str) -> usize {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let qrt = rt.clone();
        let topic = topic.to_string();
        rt.spawn(move |mut ctx| async move {
            let q = serde_json::json!({ "op": "count", "topic": topic, "reply": ctx.pid().raw() });
            qrt.send(broker, q.to_string().into_bytes());
            if let Some(m) = ctx.recv().await.message() {
                let _ = tx.send(String::from_utf8_lossy(&m).parse().unwrap_or(usize::MAX));
            }
        });
        rx.await.unwrap_or(usize::MAX)
    }

    /// Poll `pubsub_count` until it reaches `target` (or fail after a deadline).
    async fn wait_pubsub_count(rt: &Runtime, broker: rusm_otp::Pid, topic: &str, target: usize) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if pubsub_count(rt, broker, topic).await == target {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{topic:?} subscriber count never reached {target}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pubsub_fans_out_per_topic_and_prunes_dead_subscribers() {
        // The `rusm_rs::pubsub::Topics` primitive, end-to-end via a tiny broker
        // component: keyed fan-out (topic isolation) and monitor-based pruning of a
        // dead subscriber — the broker carries none of that machinery itself.
        const BROKER: &[u8] = include_bytes!("../../tests/fixtures/pubsub_broker.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(BROKER).unwrap(), "run")
            .unwrap();
        // Auto-prune monitors subscribers, so the broker needs the process-control grant.
        let _broker =
            wr.spawn_component_with(&pre, Capabilities::nothing().allow_process_control(true));
        let broker = loop {
            if let Some(pid) = rt.whereis("pubsub") {
                break pid;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        };

        // Two subscribers, each forwarding what it receives to an mpsc for assertions.
        let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let sub_a = rt.spawn(move |mut ctx| async move {
            loop {
                if let Some(m) = ctx.recv().await.message() {
                    let _ = tx_a.send(m);
                }
            }
        });
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let sub_b = rt.spawn(move |mut ctx| async move {
            loop {
                if let Some(m) = ctx.recv().await.message() {
                    let _ = tx_b.send(m);
                }
            }
        });

        let send_cmd = |v: serde_json::Value| rt.send(broker, v.to_string().into_bytes());
        send_cmd(serde_json::json!({ "op": "sub", "topic": "pages/1", "pid": sub_a.pid().raw() }));
        send_cmd(serde_json::json!({ "op": "sub", "topic": "pages/2", "pid": sub_b.pid().raw() }));
        wait_pubsub_count(&rt, broker, "pages/1", 1).await;
        wait_pubsub_count(&rt, broker, "pages/2", 1).await;

        // Fan-out is keyed: publishing to pages/1 reaches only its subscriber.
        send_cmd(serde_json::json!({ "op": "pub", "topic": "pages/1", "data": "hello" }));
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx_a.recv()).await;
        assert_eq!(got.unwrap().as_deref(), Some(b"hello".as_slice()));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), rx_b.recv())
                .await
                .is_err(),
            "topic isolation: a pages/2 subscriber must not receive pages/1 events"
        );

        // Kill sub_a → its monitor `__down` prunes it from the broker (no unsubscribe).
        rt.kill(sub_a.pid());
        wait_pubsub_count(&rt, broker, "pages/1", 0).await;

        // The surviving subscriber still receives on its own topic.
        send_cmd(serde_json::json!({ "op": "pub", "topic": "pages/2", "data": "world" }));
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), rx_b.recv()).await;
        assert_eq!(got.unwrap().as_deref(), Some(b"world".as_slice()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stock_command_component_runs() {
        // A standard `wasi:cli/run` component (no rusm:runtime, just std) runs as a RUSM
        // process and does real WASI work — here, writing a marker to a preopened dir.
        const CMD: &[u8] = include_bytes!("../../tests/fixtures/cmd_writes.wasm");
        let dir = std::env::temp_dir().join(format!("rusm-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let component = wr.compile_component(CMD).unwrap();
        let caps = CapabilityProfile::Sandboxed
            .capabilities()
            .preopen(dir.clone(), "/out", false);
        // Wait for the command process to *finish* rather than polling a deadline: it
        // writes the marker during its run, so once it has exited the marker is present.
        // This is deterministic — no timing race even when the whole wasm suite runs in
        // parallel and the command is heavily CPU-starved.
        wr.spawn_command_with(&component, caps)
            .unwrap()
            .join()
            .await;

        let marker = dir.join("ran.txt");
        let got = std::fs::read(&marker).expect("command component wrote its marker");
        assert_eq!(got, b"command component ran");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_reaches_a_server_when_granted_and_is_denied_when_sandboxed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // A minimal HTTP/1.1 server: answers every connection with a known body.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf).await; // consume the request head
                    let body = "hello from server";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });

        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                try {
                    const resp = await fetch("__URL__");
                    const text = await resp.text();
                    Process.send(replyTo, resp.ok + "|" + resp.status + "|" + text);
                } catch (e) {
                    Process.send(replyTo, "ERR:" + ((e && e.message) || e));
                }
            };
        "#;
        let bundle = BUNDLE.replace("__URL__", &format!("http://{addr}/"));

        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();

        // Granted: a NetworkClient guest fetches and reads the streamed body.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js_with(
            bundle.as_bytes(),
            CapabilityProfile::NetworkClient.capabilities(),
        );
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "true|200|hello from server",
            "a granted guest's fetch reaches the server and streams the body"
        );

        // Denied: a sandboxed guest's fetch is refused at the host (default-deny).
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        let collector2 = rt.spawn(move |mut ctx| async move {
            let _ = tx2.send(ctx.recv().await.message().unwrap());
        });
        let sandboxed = wr.spawn_js(bundle.as_bytes());
        rt.send(
            sandboxed.pid(),
            collector2.pid().raw().to_string().into_bytes(),
        );
        let denied = String::from_utf8(rx2.await.unwrap()).unwrap();
        assert!(
            denied.starts_with("ERR:"),
            "a sandboxed guest's fetch is denied; got: {denied}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_streams_a_chunked_body_frame_by_frame_without_busy_polling() {
        // The LLM-streaming shape that must NOT busy-poll: a chunked response whose
        // frames arrive with real gaps between them. The guest reads via raw
        // `wasi:io` blocking-read, which parks the fiber across each gap — so it both
        // receives every frame (correctness) and stays idle while waiting (no spin).
        // The old wstd reactor span the CPU here; this test pins the fixed behaviour.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf).await; // consume the request head
                                                         // Chunked transfer: three frames, a 120 ms gap between each — the
                                                         // guest must park (not spin) while waiting for the next frame.
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
                        .await;
                    let _ = stream.flush().await;
                    for frame in ["one ", "two ", "three"] {
                        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                        let _ = stream
                            .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
                            .await;
                        let _ = stream.flush().await;
                    }
                    let _ = stream.write_all(b"0\r\n\r\n").await; // terminating chunk
                    let _ = stream.flush().await;
                });
            }
        });

        // The guest streams the body via a reader, concatenating each frame as it
        // arrives — exercising repeated `__fetch_read` (one blocking read per call).
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                try {
                    const resp = await fetch("__URL__");
                    const reader = resp.body.getReader();
                    let out = "";
                    const dec = new TextDecoder();
                    for (;;) {
                        const { value, done } = await reader.read();
                        if (done) break;
                        out += dec.decode(value, { stream: true });
                    }
                    Process.send(replyTo, "ok|" + out);
                } catch (e) {
                    Process.send(replyTo, "ERR:" + ((e && e.message) || e));
                }
            };
        "#;
        let bundle = BUNDLE.replace("__URL__", &format!("http://{addr}/"));

        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js_with(
            bundle.as_bytes(),
            CapabilityProfile::NetworkClient.capabilities(),
        );
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ok|one two three",
            "the guest streamed every chunked frame across the inter-frame gaps"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_posts_a_request_body_to_the_server() {
        // The agent path: a POST whose JSON body must actually reach the server. Raw
        // wasi:http requires dispatching (`handle`) BEFORE streaming the body — write it
        // first and the request goes out empty and the server never replies (the hang we
        // hit with the LLM agents). This server echoes the received body back.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    // Read until the headers are in, then drain exactly the body bytes.
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    loop {
                        let n = stream.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                            let len = head
                                .split("content-length:")
                                .nth(1)
                                .and_then(|s| s.split("\r\n").next())
                                .and_then(|s| s.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if buf.len() >= i + 4 + len {
                                break;
                            }
                        }
                    }
                    let body = buf
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|i| buf[i + 4..].to_vec())
                        .unwrap_or_default();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(&body).await; // echo the received body
                    let _ = stream.flush().await;
                });
            }
        });

        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                try {
                    const resp = await fetch("__URL__", {
                        method: "POST",
                        headers: { "content-type": "application/json" },
                        body: JSON.stringify({ hello: "from-guest", n: 42 }),
                    });
                    Process.send(replyTo, "ok|" + (await resp.text()));
                } catch (e) {
                    Process.send(replyTo, "ERR:" + ((e && e.message) || e));
                }
            };
        "#;
        let bundle = BUNDLE.replace("__URL__", &format!("http://{addr}/"));

        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js_with(
            bundle.as_bytes(),
            CapabilityProfile::NetworkClient.capabilities(),
        );
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        let got = String::from_utf8(rx.await.unwrap()).unwrap();
        // The request body is sent chunked (no Content-Length), so the echo may carry
        // chunk framing — what matters is the JSON body actually reached the server.
        assert!(
            got.starts_with("ok|") && got.contains("{\"hello\":\"from-guest\",\"n\":42}"),
            "the POST body must reach the server (dispatch before write); got: {got:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_reads_granted_env_via_process_env() {
        // `process.env.<KEY>` reads a capability-granted env var; an ungranted key is
        // `undefined` and `in` reflects presence — the config path TS guests (e.g. an
        // LLM agent reading ANTHROPIC_API_KEY) rely on.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const present = process.env.AGENT_KEY;
                const missing = process.env.NOT_GRANTED;
                Process.send(replyTo, `${present}|${missing}|${"AGENT_KEY" in process.env}`);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // Grant exactly one key, with a value; the other is never granted.
        let caps = CapabilityProfile::Sandboxed
            .capabilities()
            .env("AGENT_KEY", "sk-test");
        let guest = wr.spawn_js_with(BUNDLE.as_bytes(), caps);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "sk-test|undefined|true",
            "granted env is read; ungranted is undefined; `in` reflects presence"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_has_base64_btoa_atob() {
        // The standard Web base64 codecs: `btoa`/`atob` over Latin-1 binary strings,
        // round-tripping (the JWT / data-URL / hashing path the ecosystem expects).
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const enc = btoa("hi");                 // "aGk="
                const dec = atob("aGk=");               // "hi"
                const roundtrip = atob(btoa("RUSMÿ")) === "RUSMÿ";
                Process.send(replyTo, `${enc}|${dec}|${roundtrip}`);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "aGk=|hi|true",
            "btoa/atob encode, decode, and round-trip binary"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_handles_binary_messages() {
        // JS receives a reply-to (text) and a binary message (Uint8Array), then
        // echoes the bytes back — proving binary marshalling both ways.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const bytes = await Process.receive();   // Uint8Array
                Process.send(replyTo, bytes);            // send it back (binary path)
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        rt.send(guest.pid(), vec![7, 8, 9]);
        assert_eq!(rx.await.unwrap(), vec![7, 8, 9]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_consumes_a_byte_stream() {
        // JS accepts a stream, reads Uint8Array chunks to EOF, reports the total.
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const collector = await Process.receiveText();
                const s = Process.acceptStream();
                let total = 0, chunk;
                while ((chunk = await s.read()) !== null) { total += chunk.length; }
                Process.send(collector, "total:" + total);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_js(BUNDLE.as_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());

        // Stream 3x "hello!" (18 bytes) into the JS consumer, then EOF.
        let (writer, reader) = rusm_otp::stream();
        rt.send_stream(guest.pid(), reader);
        for _ in 0..3 {
            writer.write(b"hello!".to_vec()).await.unwrap();
        }
        drop(writer); // end of stream

        let reply = String::from_utf8(rx.await.unwrap()).unwrap();
        assert_eq!(reply, "total:18", "JS read all streamed bytes to EOF");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_service_dispatches_exported_functions() {
        // A service component EXPORTS functions (no Process plumbing); the runner
        // runs the request→dispatch→reply loop. A Rust "client" drives it: send a
        // JSON request, get a JSON reply. Proves the service half of the typed RPC.
        const SERVICE: &str = r#"
            module.exports.add = (a, b) => a + b;
            module.exports.greet = async ({ name }) => "hi " + name;   // async handler too
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let svc = wr.spawn_js(SERVICE.as_bytes());
        // request: call `add(2, 3)`, asking `collector` to be answered with ref 1.
        let req = format!(
            r#"{{"op":"add","args":[2,3],"from":"{}","ref":1}}"#,
            collector.pid().raw()
        );
        rt.send(svc.pid(), req.into_bytes());

        let reply = String::from_utf8(rx.await.unwrap()).unwrap();
        assert!(
            reply.contains("\"ref\":1") && reply.contains("\"ok\":5"),
            "service should reply {{ref:1, ok:5}}, got {reply}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_commander_calls_a_service_via_the_typed_client() {
        // The whole concealed-function-call story end to end, in JS: a `calc`
        // service exports `add`; the commander spawns it BY NAME and `await`s a
        // typed call — spawn + send + receive all hidden by the client proxy.
        const CALC: &str = r#"module.exports.add = (a, b) => a + b;"#;
        const COMMANDER: &str = r#"
            module.exports.default = async function () {
                const collector = await Process.receiveText();
                const calc = spawn("calc");          // spawn-from-guest by name
                const sum = await calc.add(2, 3);    // concealed call: send + await reply
                Process.send(collector, "sum=" + sum);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        // Register the service by name so the commander can spawn it.
        wr.register_js_component("calc", CALC.as_bytes());

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // The commander needs the spawn capability (Trusted grants it).
        let commander = wr.spawn_js_with(
            COMMANDER.as_bytes(),
            CapabilityProfile::Trusted.capabilities(),
        );
        rt.send(
            commander.pid(),
            collector.pid().raw().to_string().into_bytes(),
        );

        let reply = String::from_utf8(rx.await.unwrap()).unwrap();
        assert_eq!(
            reply, "sum=5",
            "typed client called the service and got 2+3"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typed_client_streams_a_generator_handler() {
        // A service exports an async generator; the commander `for await`s the typed
        // call — chunks ride a RUSM byte stream under the hood, surfaced as values.
        const COUNTER: &str = r#"
            module.exports.count = async function* (n) { for (let i = 0; i < n; i++) yield i; };
        "#;
        const COMMANDER: &str = r#"
            module.exports.default = async function () {
                const collector = await Process.receiveText();
                const c = spawn("counter");
                const acc = [];
                for await (const x of c.count(3)) acc.push(x);   // streaming typed call
                Process.send(collector, "got:" + acc.join(","));
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_component("counter", COUNTER.as_bytes());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let commander = wr.spawn_js_with(
            COMMANDER.as_bytes(),
            CapabilityProfile::Trusted.capabilities(),
        );
        rt.send(
            commander.pid(),
            collector.pid().raw().to_string().into_bytes(),
        );
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "got:0,1,2",
            "the generator's yielded chunks streamed through the typed client"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typed_client_passes_a_callback() {
        // A function argument becomes a callback: it stays in the caller, and the
        // service's invocations travel back as messages the client routes to it.
        const WORKER: &str = r#"
            module.exports.work = async function (onProgress) {
                onProgress(1); onProgress(2);
                return "ok";
            };
        "#;
        const COMMANDER: &str = r#"
            module.exports.default = async function () {
                const collector = await Process.receiveText();
                const w = spawn("worker");
                const log = [];
                const r = await w.work((n) => log.push(n));   // callback stays home
                Process.send(collector, r + ":" + log.join(","));
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_component("worker", WORKER.as_bytes());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let commander = wr.spawn_js_with(
            COMMANDER.as_bytes(),
            CapabilityProfile::Trusted.capabilities(),
        );
        rt.send(
            commander.pid(),
            collector.pid().raw().to_string().into_bytes(),
        );
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ok:1,2",
            "the callback ran in the caller as the service invoked it"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_guest_uses_the_rusm_rs_api() {
        // A component written with the ergonomic `rusm-rs` crate (the Rust guest
        // API + the wit-bindgen library/binary split): it receives a reply-to pid,
        // labels itself, and answers — all via `rusm_rs::*`.
        const RS_GUEST: &[u8] = include_bytes!("../../tests/fixtures/rs_guest.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(RS_GUEST).unwrap(), "run")
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            format!("hello from {}", guest.pid().raw()),
            "the Rust guest drove the actor API through rusm-rs"
        );
    }

    /// Host bindings for the **custom application bridge** the `custom_bridge` fixture
    /// imports — generated from the same `demo:bridge` WIT the fixture declares. This
    /// mirrors how an app's own `bridges/<name>/host.rs` would `bindgen!` its contract
    /// and implement it over [`crate::BridgeHost`].
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

    impl greet_bridge::demo::bridge::greet::Host for crate::BridgeHost {
        async fn greet(&mut self, name: String) -> String {
            // Reaches real process state through the curated public accessor — the custom
            // bridge gets the same host context the built-in bridges do, nothing more.
            format!("hello, {name} from {}", self.pid())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_custom_application_bridge_is_callable_from_a_guest() {
        // The load-bearing seam for app-authored bridges: `WasmRuntime::with_bridges`
        // wires a custom *typed* host function into the component linker, and a guest
        // calls it as an ordinary WIT import — no dispatcher, typed end to end. This is
        // exactly the surface a `bridges/<name>/host.rs` in an application repo uses.
        const CUSTOM_BRIDGE: &[u8] = include_bytes!("../../tests/fixtures/custom_bridge.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::with_bridges(rt.clone(), |linker| {
            greet_bridge::demo::bridge::greet::add_to_linker::<
                _,
                wasmtime::component::HasSelf<crate::BridgeHost>,
            >(linker, |host| host)
        })
        .unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(CUSTOM_BRIDGE).unwrap(), "run")
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            format!("hello, World from {}", guest.pid().raw()),
            "the guest called the app-registered custom host bridge and got its typed reply"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_builder_composes_a_store_with_custom_bridges() {
        // The composable construction path: ONE runtime carrying BOTH a durable store and
        // a custom application bridge — the combination the fixed `with_*` constructors
        // can't express. Proves they coexist in a single runtime: a guest granted storage
        // runs the full kv CRUD against the store, and the custom-bridge guest calls its
        // host function — both succeed.
        const CUSTOM_BRIDGE: &[u8] = include_bytes!("../../tests/fixtures/custom_bridge.wasm");
        let rt = Runtime::new();
        let path = kv_test_path("builder-bridges");
        let wr = WasmRuntime::builder(rt.clone())
            .store(&path)
            .bridges(|linker| {
                greet_bridge::demo::bridge::greet::add_to_linker::<
                    _,
                    wasmtime::component::HasSelf<crate::BridgeHost>,
                >(linker, |host| host)
            })
            .build()
            .unwrap();

        // The store backs the full kv CRUD from a guest granted storage.
        let flags =
            run_kv_fixture(&wr, &rt, CapabilityProfile::Trusted.capabilities(), RS_KV).await;
        assert_eq!(
            flags, 0b11_1111,
            "the builder's store backs the full kv CRUD"
        );

        // The custom bridge is callable in the same runtime.
        let pre = wr
            .prepare_component(&wr.compile_component(CUSTOM_BRIDGE).unwrap(), "run")
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            format!("hello, World from {}", guest.pid().raw()),
            "the builder's custom bridge is callable alongside the store"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_guest_uses_the_actor_world() {
        // A component written in Go (TinyGo → wasm32-wasip2 component) over the same
        // `rusm:runtime` actor world: it receives a reply-to pid, labels itself, and
        // answers — proving TinyGo's component output links the host actor imports and
        // that a synchronous import blocking on the host fiber (`receive`) works.
        const GO_GUEST: &[u8] = include_bytes!("../../tests/fixtures/go_guest.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(GO_GUEST).unwrap(), "run")
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            format!("hello from {}", guest.pid().raw()),
            "the Go guest drove the actor API through the wit-bindgen-go bindings"
        );
    }

    #[test]
    fn the_go_sdk_vendors_the_canonical_actor_wit() {
        // The rusm-go SDK vendors the actor WIT so TinyGo / wit-bindgen-go can bind it;
        // it must stay byte-identical to the canonical rusm-rs world (the single source
        // of truth). If this fails, re-copy crates/rusm-rs/wit/world.wit over the SDK's
        // copy. (The SDK adds the WASI-embedding `component` world in a separate file.)
        assert_eq!(
            include_str!("../../../../packages/rusm-go/wit/world.wit"),
            include_str!("../../../rusm-rs/wit/world.wit"),
            "packages/rusm-go/wit/world.wit drifted from rusm-rs/wit/world.wit",
        );
    }

    #[test]
    fn the_js_http_runner_vendors_the_canonical_actor_wit() {
        // The js-http-runner imports the actor world so a TS `fetch` handler gets `kv` +
        // the publish ops + platform logging; it vendors the actor WIT as a dep, which
        // must stay byte-identical to the canonical rusm-rs world (the single source of
        // truth). If this fails, re-copy crates/rusm-rs/wit/world.wit over the dep copy.
        assert_eq!(
            include_str!("../../js-http-runner/wit/deps/rusm-runtime/world.wit"),
            include_str!("../../../rusm-rs/wit/world.wit"),
            "js-http-runner/wit/deps/rusm-runtime/world.wit drifted from rusm-rs/wit/world.wit",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_guest_logs_via_slog_and_the_standard_log_package() {
        // The Go guest logs the normal way — log/slog (with attrs and a group) and the
        // standard log package — and the SDK routes all of it to the host log op. If any
        // path trapped (the slog handler, WithGroup/WithAttrs, or the log writer) the
        // guest would never reply; the reply proves the whole guest → host log bridge
        // ran. (The host gates output on the node's [log] level; the bridge runs either
        // way. Mirrors the TS console test.)
        const GO_LOG: &[u8] = include_bytes!("../../tests/fixtures/go_log.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(GO_LOG).unwrap(), "run")
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(String::from_utf8(rx.await.unwrap()).unwrap(), "logged ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_service_speaks_the_cross_language_wire() {
        // A Go service (Fn2 / Fn1 over rusm.Service) answers requests whose bytes are
        // exactly what a rusm-rs / rusm-ts client emits — {op,args,from,ref} →
        // {ref,ok} | {ref,err} — proving Go's serve loop and wire codec are byte-
        // compatible across languages (so RS/TS clients interoperate with a Go service).
        const GO_SERVICE: &[u8] = include_bytes!("../../tests/fixtures/go_service.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(GO_SERVICE).unwrap(), "run")
            .unwrap();
        let service = wr.spawn_component(&pre);
        let service_pid = service.pid();

        // Send exactly the wire a typed client emits and collect the service's reply.
        let exchange = |op: &'static str, args: &'static str, reference: u64| {
            let rt = &rt;
            async move {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let collector = rt.spawn(move |mut ctx| async move {
                    let _ = tx.send(ctx.recv().await.message().unwrap());
                });
                let req = format!(
                    r#"{{"op":"{op}","args":{args},"from":"{}","ref":{reference}}}"#,
                    collector.pid().raw()
                );
                rt.send(service_pid, req.into_bytes());
                String::from_utf8(rx.await.unwrap()).unwrap()
            }
        };

        assert_eq!(
            exchange("add", "[2,3]", 1).await,
            r#"{"ref":1,"ok":5}"#,
            "the Fn2 handler decoded positional args and returned the canonical reply"
        );
        assert_eq!(
            exchange("greet", r#"["ada"]"#, 2).await,
            r#"{"ref":2,"ok":"hi ada"}"#,
            "the Fn1 handler returned the canonical reply"
        );
        assert_eq!(
            exchange("nope", "[]", 3).await,
            r#"{"ref":3,"err":"no such function: nope"}"#,
            "an unknown op is reported as an error reply"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_client_calls_a_go_service_via_the_typed_call() {
        // A Go client (Call[int] / Call[string]) drives a Go service over the same wire
        // the cross-language test pins — closing the loop on the typed client: it encodes
        // requests, matches the reply by id, and decodes typed results.
        const GO_SERVICE: &[u8] = include_bytes!("../../tests/fixtures/go_service.wasm");
        const GO_CLIENT: &[u8] = include_bytes!("../../tests/fixtures/go_client.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let svc = wr
            .prepare_component(&wr.compile_component(GO_SERVICE).unwrap(), "run")
            .unwrap();
        let cli = wr
            .prepare_component(&wr.compile_component(GO_CLIENT).unwrap(), "run")
            .unwrap();
        let service = wr.spawn_component(&svc);
        let client = wr.spawn_component(&cli);

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // The client expects the service pid, then the reply-to pid.
        rt.send(client.pid(), service.pid().raw().to_string().into_bytes());
        rt.send(client.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "sum=5 greet=hi ada count=[1 2 3] countup=done ticks=[1 2 3]",
            "the Go client called methods, ranged the stream, and ran a callback"
        );
    }

    /// A worker that answers one call by hand: first a colliding-ref REQUEST (op/from, no
    /// ok/err — the trap), then the genuine reply. Shared by the collision tests below as the
    /// noisy peer that reproduces the wire `ref`-collision race.
    const NOISY_ECHOER: &str = r#"
        module.exports.default = async function () {
            const req = JSON.parse(await Process.receiveText());
            const from = BigInt(req.from);
            Process.send(from, JSON.stringify({ op: "noise", from: Process.self().toString(), ref: req.ref }));
            Process.send(from, JSON.stringify({ ref: req.ref, ok: "echoed:" + req.args[0] }));
        };
    "#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_call_isnt_fooled_by_a_colliding_ref_request_during_the_wait() {
        // Regression for the wire `ref`-collision (the bug genius-rusm hit): while a client
        // awaits its reply (ref 1), a concurrent inbound *request* that happens to carry the
        // same per-process ref must NOT be mistaken for the reply (bug 1), nor re-read forever
        // off the stash (bug 2). The echoer hand-crafts exactly that race: a request-shaped
        // message carrying the call's ref, sent just before the real reply. Drives the real
        // js-runner (the rpc.js fix), the cross-guest twin of the rusm-rs `classify_reply`
        // unit tests.
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();

        // The client uses the typed `spawn().method()` path — exercising `__call`.
        const CLIENT: &str = r#"
            module.exports.default = async function () {
                const collector = BigInt(await Process.receiveText());
                const r = await spawn("echoer").echo("hi");
                Process.send(collector, "got:" + r);
            };
        "#;
        wr.register_js_component("echoer", NOISY_ECHOER.as_bytes().to_vec());

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // The client needs `spawn` (Trusted grants it).
        let client = wr.spawn_js_with(CLIENT.as_bytes(), CapabilityProfile::Trusted.capabilities());
        rt.send(client.pid(), collector.pid().raw().to_string().into_bytes());

        let got = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("client must not hang on the colliding request (bug 2: stash re-read loop)")
            .unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "got:echoed:hi",
            "the colliding request was set aside and the real reply returned (bug 1)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_call_isnt_fooled_by_a_colliding_ref_request() {
        // The Go twin: a real Go guest's typed `Call` must set the colliding-ref request aside
        // and return the genuine reply — driven against the same noisy JS echoer, so it also
        // proves cross-language wire correctness.
        const GO_CLIENT: &[u8] = include_bytes!("../../tests/fixtures/go_collision_client.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let echoer = wr.spawn_js(NOISY_ECHOER.as_bytes());
        let cli = wr
            .prepare_component(&wr.compile_component(GO_CLIENT).unwrap(), "run")
            .unwrap();
        let client = wr.spawn_component(&cli);

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // The client expects the echoer pid, then the collector pid.
        rt.send(client.pid(), echoer.pid().raw().to_string().into_bytes());
        rt.send(client.pid(), collector.pid().raw().to_string().into_bytes());

        let got = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("the Go client must not hang on the colliding request")
            .unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "got:echoed:hi",
            "Go Call set the colliding request aside and returned the real reply"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_ts_guest_connects_to_a_resident_by_name_and_calls_it() {
        // rusm-ts `connect(name)`: a TS caller reaches an EXISTING resident by its registered
        // name and makes a typed call — clean app code, no hand-rolled wire envelope (the TS
        // twin of Rust's `Client::connect` / Go's `Call(pid, …)`).
        const COUNTER: &str = r#"
            let count = 0;
            module.exports.bump  = (by) => { count += by; return count; };
            module.exports.total = ()   => count;
        "#;
        const CALLER: &str = r#"
            module.exports.default = async () => {
                const collector = BigInt(await Process.receiveText());
                const c = connect("counter");          // reach the running resident by name
                const a = await c.bump(2);
                const b = await c.bump(3);             // its in-memory state persists across calls
                Process.send(collector, "total:" + b + " first:" + a);
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let counter = wr.spawn_js(COUNTER.as_bytes()); // the resident service
        rt.register("counter", counter.pid()); // registered under its name (a resident self-registers)

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let caller = wr.spawn_js_with(CALLER.as_bytes(), CapabilityProfile::Trusted.capabilities());
        rt.send(caller.pid(), collector.pid().raw().to_string().into_bytes());

        let got = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("the connect()ed call must return")
            .unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "total:5 first:2",
            "connect(name) reached the resident; its in-memory state persisted across calls"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rs_http_fetch_reaches_a_server_when_granted_and_is_denied_when_sandboxed() {
        // The `rusm_rs::http::fetch` SDK helper (raw wasi:http, capability-gated) — a Rust guest
        // does outbound HTTP without hand-rolling the plumbing. Granted → reaches the server;
        // sandboxed → refused at the host. (The Rust twin of the JS `fetch` test above.)
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf).await; // consume the request head
                    let body = "hello from server";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });

        const RS_FETCH: &[u8] = include_bytes!("../../tests/fixtures/rs_fetch.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let prepared = wr
            .prepare_component(&wr.compile_component(RS_FETCH).unwrap(), "run")
            .unwrap();
        let url = format!("http://{addr}/");

        // Granted: a NetworkClient guest fetches and reads the body.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest =
            wr.spawn_component_with(&prepared, CapabilityProfile::NetworkClient.capabilities());
        rt.send(guest.pid(), url.clone().into_bytes());
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("fetch must return")
            .unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "200|hello from server",
            "a granted guest's http::fetch reaches the server and reads the body"
        );

        // Denied: a sandboxed guest's fetch is refused at the host (default-deny).
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        let collector2 = rt.spawn(move |mut ctx| async move {
            let _ = tx2.send(ctx.recv().await.message().unwrap());
        });
        let sandboxed =
            wr.spawn_component_with(&prepared, CapabilityProfile::Sandboxed.capabilities());
        rt.send(sandboxed.pid(), url.into_bytes());
        rt.send(
            sandboxed.pid(),
            collector2.pid().raw().to_string().into_bytes(),
        );
        let denied = String::from_utf8(
            tokio::time::timeout(std::time::Duration::from_secs(10), rx2)
                .await
                .expect("denied fetch must return")
                .unwrap(),
        )
        .unwrap();
        assert!(
            denied.starts_with("ERR:"),
            "a sandboxed guest's http::fetch is denied; got: {denied}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_supervisor_restarts_a_dead_child() {
        // rusm.Supervisor (one-for-one) over the host's native supervise ABI: it spawns
        // + monitors the `flaky` Go child, which announces its pid to the registered
        // collector. Killing the child makes the supervisor restart it as a fresh process.
        const FLAKY: &[u8] = include_bytes!("../../tests/fixtures/go_flaky.wasm");
        const SUP: &[u8] = include_bytes!("../../tests/fixtures/go_sup.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let flaky = wr
            .prepare_component(&wr.compile_component(FLAKY).unwrap(), "run")
            .unwrap();
        let sup = wr
            .prepare_component(&wr.compile_component(SUP).unwrap(), "run")
            .unwrap();
        wr.register_component("flaky", flaky);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());

        // The supervisor needs spawn + monitor (Trusted grants both).
        let _sup = wr.spawn_component_with(&sup, CapabilityProfile::Trusted.capabilities());

        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let pid_a = parse(rx.recv().await.unwrap());
        rt.kill(rusm_otp::Pid::from_raw(pid_a)); // the supervisor should restart it
        let pid_b = parse(rx.recv().await.unwrap());
        assert_ne!(
            pid_a, pid_b,
            "the Go supervisor restarted the dead child as a fresh process"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_supervisor_gives_up_past_its_restart_intensity() {
        // go-sup allows 2 restarts within an hour. A rapid burst of kills exceeds that
        // intensity, so the supervisor gives up and its own process exits (the failure
        // escalates instead of restart-looping) — proving the MaxRestarts/Within fields
        // flow from the Go struct into the host's supervise ABI.
        const FLAKY: &[u8] = include_bytes!("../../tests/fixtures/go_flaky.wasm");
        const SUP: &[u8] = include_bytes!("../../tests/fixtures/go_sup.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let flaky = wr
            .prepare_component(&wr.compile_component(FLAKY).unwrap(), "run")
            .unwrap();
        let sup = wr
            .prepare_component(&wr.compile_component(SUP).unwrap(), "run")
            .unwrap();
        wr.register_component("flaky", flaky);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());

        let sup_handle = wr.spawn_component_with(&sup, CapabilityProfile::Trusted.capabilities());
        let sup_pid = sup_handle.pid();

        // Initial start + 2 restarts = 3 starts; kill the child each time it appears.
        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let mut starts = Vec::new();
        for _ in 0..3 {
            let pid = parse(rx.recv().await.unwrap());
            starts.push(pid);
            rt.kill(rusm_otp::Pid::from_raw(pid));
        }

        // Past its restart intensity, the supervisor gives up — its own process ends.
        let mut gave_up = false;
        for _ in 0..400 {
            if !rt.is_alive(sup_pid) {
                gave_up = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            gave_up,
            "the Go supervisor exited after exceeding restart intensity"
        );
        assert_eq!(
            starts
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "each restart was a fresh process"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_rust_guest_drives_process_group_tags() {
        // End-to-end through the guest ABI: a guest tags itself (register_tag), the host
        // sees the group, and kill_tag terminates it — gated by process-control.
        const RS_TAG: &[u8] = include_bytes!("../../tests/fixtures/rs_tag.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(RS_TAG).unwrap(), "run")
            .unwrap();

        // Spawn a guest under `caps`, hand it a reply-to collector + a command, and return
        // its handle plus a receiver for its single reply.
        let drive = |caps: crate::Capabilities, cmd: &str| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let collector = rt.spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await.message().unwrap());
            });
            let g = wr.spawn_component_with(&pre, caps);
            rt.send(g.pid(), collector.pid().raw().to_string().into_bytes());
            rt.send(g.pid(), cmd.as_bytes().to_vec());
            (g, rx)
        };

        // Two taggers join "grp" via the guest's register_tag; each acks "tagged".
        let (t1, r1) = drive(CapabilityProfile::Sandboxed.capabilities(), "tag grp");
        let (t2, r2) = drive(CapabilityProfile::Sandboxed.capabilities(), "tag grp");
        assert_eq!(r1.await.unwrap(), b"tagged");
        assert_eq!(r2.await.unwrap(), b"tagged");
        assert_eq!(
            rt.whereis_tag("grp").len(),
            2,
            "the guests' register_tag reached the runtime through the ABI"
        );

        // A sandboxed killer cannot terminate the group (process-control denied) → 0.
        let (denied, rd) = drive(CapabilityProfile::Sandboxed.capabilities(), "kill grp");
        assert_eq!(
            rd.await.unwrap(),
            b"0",
            "kill_tag is denied without process-control"
        );
        denied.join().await;
        assert_eq!(
            rt.whereis_tag("grp").len(),
            2,
            "the group survived the denied kill"
        );

        // A process-control killer terminates the whole group.
        let (killer, rk) = drive(CapabilityProfile::Trusted.capabilities(), "kill grp");
        assert_eq!(rk.await.unwrap(), b"2", "kill_tag terminated both members");
        killer.join().await;
        t1.join().await;
        t2.join().await;
        assert!(
            rt.whereis_tag("grp").is_empty(),
            "killed members are reaped out of the group"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_builder_js_runner_override_is_used() {
        // A per-app js-runner is supplied via the builder (`rusm build` rebuilds it with an
        // app's custom bridges compiled in). Supplying the embedded bytes as an explicit
        // override must behave exactly like the default — proving the override path is wired
        // (the actual custom-bridge runner is exercised by the example end to end).
        const ECHO: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                Process.send(replyTo, "ok");
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::builder(rt.clone())
            .js_runner(crate::JS_RUNNER_WASM.to_vec())
            .build()
            .unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let g = wr.spawn_js(ECHO.as_bytes().to_vec());
        rt.send(g.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            rx.await.unwrap(),
            b"ok",
            "the overridden js-runner ran the guest"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_ts_guest_drives_process_group_tags() {
        // The TS twin of the Rust tag test, through the rquickjs bridge: Process.registerTag
        // / Process.killTag, with the same gating (sandboxed killer denied → 0).
        const TAGGER: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                Process.registerTag("tsgrp");
                Process.send(replyTo, "tagged");
                for (;;) await Process.receive(); // stay alive in the group
            };
        "#;
        const KILLER: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const tag = await Process.receiveText();
                Process.send(replyTo, String(Process.killTag(tag)));
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();

        // Spawn a JS guest under `caps` (bundle is message 1), hand it a reply-to collector
        // then any extra messages, and return its handle + a receiver for its reply.
        let drive = |bundle: &str, caps: crate::Capabilities, then: &[&[u8]]| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let collector = rt.spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await.message().unwrap());
            });
            let g = wr.spawn_js_with(bundle.as_bytes().to_vec(), caps);
            rt.send(g.pid(), collector.pid().raw().to_string().into_bytes());
            for m in then {
                rt.send(g.pid(), m.to_vec());
            }
            (g, rx)
        };

        // Two TS taggers join "tsgrp" via Process.registerTag; each acks "tagged".
        let (t1, r1) = drive(TAGGER, CapabilityProfile::Sandboxed.capabilities(), &[]);
        let (t2, r2) = drive(TAGGER, CapabilityProfile::Sandboxed.capabilities(), &[]);
        assert_eq!(r1.await.unwrap(), b"tagged");
        assert_eq!(r2.await.unwrap(), b"tagged");
        assert_eq!(
            rt.whereis_tag("tsgrp").len(),
            2,
            "TS registerTag reached the runtime"
        );

        // A sandboxed killer is denied (process-control) → 0; the group survives.
        let (denied, rd) = drive(
            KILLER,
            CapabilityProfile::Sandboxed.capabilities(),
            &[b"tsgrp"],
        );
        assert_eq!(
            rd.await.unwrap(),
            b"0",
            "killTag denied without process-control"
        );
        denied.join().await;
        assert_eq!(rt.whereis_tag("tsgrp").len(), 2);

        // A process-control killer terminates the whole group.
        let (killer, rk) = drive(
            KILLER,
            CapabilityProfile::Trusted.capabilities(),
            &[b"tsgrp"],
        );
        assert_eq!(rk.await.unwrap(), b"2", "killTag terminated both members");
        killer.join().await;
        t1.join().await;
        t2.join().await;
        assert!(rt.whereis_tag("tsgrp").is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_guest_receives_with_a_timeout_via_rusm_rs() {
        // The ergonomic `rusm-rs` helpers: `receive_bytes_timeout` (idle → None)
        // and `receive_timeout::<String>` (JSON message before the deadline). Same
        // handshake as the raw-ABI `actor-timeout` fixture and the TS twin.
        const RS_TIMEOUT: &[u8] = include_bytes!("../../tests/fixtures/rs_timeout.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(RS_TIMEOUT).unwrap(), "run")
            .unwrap();
        let guest = wr.spawn_component(&pre);
        let guest_pid = guest.pid();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let driver_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            driver_rt.send(guest_pid, ctx.pid().raw().to_string().into_bytes());
            assert_eq!(ctx.recv().await.message().unwrap(), b"armed");
            // JSON-encode "ping" so the guest's `receive_timeout::<String>` deserializes it.
            driver_rt.send(guest_pid, serde_json::to_vec("ping").unwrap());
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        assert_eq!(
            rx.await.unwrap(),
            vec![0b11],
            "rusm-rs receive helpers must time out when idle and deliver a typed message before the deadline"
        );
        guest.join().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_supervisor_restarts_a_dead_child() {
        // The TS `supervise()` helper (one-for-one): a `flaky` worker announces its
        // pid to the collector then waits; killing it makes the supervisor restart it.
        const FLAKY: &str = r#"
            module.exports.default = async function () {
                const c = Process.whereis("collector");   // null, or a bigint pid (may be 0n)
                if (c !== null) Process.send(c, "started:" + Process.self());
                for (;;) await Process.receive();
            };
        "#;
        const SUP: &str = r#"
            module.exports.default = async function () {
                await supervise({ strategy: "one_for_one", children: ["flaky"] });
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_component("flaky", FLAKY.as_bytes());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());
        let _sup = wr.spawn_js_with(SUP.as_bytes(), CapabilityProfile::Trusted.capabilities());

        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let pid_a = parse(rx.recv().await.unwrap());
        rt.kill(rusm_otp::Pid::from_raw(pid_a));
        let pid_b = parse(rx.recv().await.unwrap());
        assert_ne!(pid_a, pid_b, "the TS supervisor restarted the dead child");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_supervisor_gives_up_past_its_restart_intensity() {
        // Parity with the Rust supervisor: `supervise({ maxRestarts, maxSeconds })`
        // gives up once a burst exceeds the restart intensity, so its process exits.
        const FLAKY: &str = r#"
            module.exports.default = async function () {
                const c = Process.whereis("collector");
                if (c !== null) Process.send(c, "started:" + Process.self());
                for (;;) await Process.receive();
            };
        "#;
        const SUP: &str = r#"
            module.exports.default = async function () {
                await supervise({ strategy: "one_for_one", children: ["flaky"], maxRestarts: 2, maxSeconds: 3600 });
            };
        "#;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_component("flaky", FLAKY.as_bytes());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());
        let sup = wr.spawn_js_with(SUP.as_bytes(), CapabilityProfile::Trusted.capabilities());
        let sup_pid = sup.pid();

        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let mut starts = Vec::new();
        for _ in 0..3 {
            let pid = parse(rx.recv().await.unwrap());
            starts.push(pid);
            rt.kill(rusm_otp::Pid::from_raw(pid));
        }

        let mut gave_up = false;
        for _ in 0..400 {
            if !rt.is_alive(sup_pid) {
                gave_up = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            gave_up,
            "TS supervisor exited after exceeding restart intensity"
        );
        assert_eq!(
            starts
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "each restart was a fresh process"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_supervisor_restarts_a_dead_child() {
        // `rusm_rs::Supervisor` (one-for-one): it spawns + monitors the `flaky`
        // child, which announces its pid to the registered collector. Killing the
        // child makes the supervisor restart it as a fresh process.
        const FLAKY: &[u8] = include_bytes!("../../tests/fixtures/rs_flaky.wasm");
        const SUP: &[u8] = include_bytes!("../../tests/fixtures/rs_sup.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let flaky = wr
            .prepare_component(&wr.compile_component(FLAKY).unwrap(), "run")
            .unwrap();
        let sup = wr
            .prepare_component(&wr.compile_component(SUP).unwrap(), "run")
            .unwrap();
        wr.register_component("flaky", flaky);

        // A collector forwards each "started:<pid>" announcement to a channel.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());

        // The supervisor needs spawn + monitor (Trusted grants both).
        let _sup = wr.spawn_component_with(&sup, CapabilityProfile::Trusted.capabilities());

        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let pid_a = parse(rx.recv().await.unwrap());
        rt.kill(rusm_otp::Pid::from_raw(pid_a)); // the supervisor should restart it
        let pid_b = parse(rx.recv().await.unwrap());
        assert_ne!(
            pid_a, pid_b,
            "the supervisor restarted the dead child as a fresh process"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_supervisor_gives_up_past_its_restart_intensity() {
        // `rs-sup` allows 2 restarts within an hour. A rapid burst of kills exceeds
        // that intensity, so the supervisor gives up and its own process exits —
        // letting the failure escalate instead of restart-looping forever.
        const FLAKY: &[u8] = include_bytes!("../../tests/fixtures/rs_flaky.wasm");
        const SUP: &[u8] = include_bytes!("../../tests/fixtures/rs_sup.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let flaky = wr
            .prepare_component(&wr.compile_component(FLAKY).unwrap(), "run")
            .unwrap();
        let sup = wr
            .prepare_component(&wr.compile_component(SUP).unwrap(), "run")
            .unwrap();
        wr.register_component("flaky", flaky);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(String::from_utf8(b).unwrap());
                }
            }
        });
        rt.register("collector", collector.pid());

        let sup_handle = wr.spawn_component_with(&sup, CapabilityProfile::Trusted.capabilities());
        let sup_pid = sup_handle.pid();

        // Initial start + 2 restarts = 3 starts; kill the child each time it appears.
        // The 3rd death is the one that exceeds the intensity.
        let parse = |m: String| -> u64 { m.strip_prefix("started:").unwrap().parse().unwrap() };
        let mut starts = Vec::new();
        for _ in 0..3 {
            let pid = parse(rx.recv().await.unwrap());
            starts.push(pid);
            rt.kill(rusm_otp::Pid::from_raw(pid));
        }

        // Past its restart intensity, the supervisor gives up — its own process ends.
        let mut gave_up = false;
        for _ in 0..400 {
            if !rt.is_alive(sup_pid) {
                gave_up = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            gave_up,
            "supervisor exited after exceeding restart intensity"
        );
        assert_eq!(
            starts
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "each restart was a fresh process"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_service_macro_dispatches_and_a_typed_client_calls_it() {
        // `#[rusm_rs::service] mod calc` → a serve() dispatch loop + a typed Client.
        // One component plays both roles: the commander spawns a sibling `calc` by
        // name and calls `add`/`greet` through the generated client.
        const RS_SERVICE: &[u8] = include_bytes!("../../tests/fixtures/rs_service.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(RS_SERVICE).unwrap(), "run")
            .unwrap();
        // Register it by name so the commander's Client::spawn("calc") resolves.
        wr.register_component("calc", pre.clone());

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        // The commander needs the spawn capability (Trusted grants it).
        let commander = wr.spawn_component_with(&pre, CapabilityProfile::Trusted.capabilities());
        rt.send(
            commander.pid(),
            collector.pid().raw().to_string().into_bytes(),
        );

        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "sum=5 hi RUSM count=1,2,3 work=done after 25/50/100",
            "the typed client called the macro service (call + streaming + callback)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn component_stream_errors_are_reported_not_fatal() {
        // role 2: open to a dead pid, write/read bogus handles — each must return
        // none/false cleanly (flags 0b111), never trap.
        const PIPE: &[u8] = include_bytes!("../../tests/fixtures/stream_pipe.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(PIPE).unwrap(), "run")
            .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let guest = wr.spawn_component(&pre);
        let mut msg = vec![2u8];
        msg.extend_from_slice(&collector.pid().raw().to_le_bytes());
        rt.send(guest.pid(), msg);

        let flags = rx.await.unwrap();
        assert_eq!(
            u32::from_le_bytes(flags[..4].try_into().unwrap()),
            0b111,
            "open-to-dead/write-bogus/read-bogus should each fail gracefully"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_memory_cap_crashes_a_component_that_grows_past_it() {
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(COMP_GROW).unwrap(), "run")
            .unwrap();
        // Cap at one page: the two-page growth is denied → the guest traps.
        let caps = Capabilities::nothing().max_memory(64 << 10);
        let guest = wr.spawn_component_with(&pre, caps);
        assert_eq!(exit_reason_of(&rt, &guest).await, ExitReason::Crashed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_component_drives_the_whole_actor_abi() {
        // A real Rust component (built for wasm32-wasip2, checked in) that calls
        // every rusm:runtime actor op and reports which succeeded.
        const ECHO: &[u8] = include_bytes!("../../tests/fixtures/actor_echo.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(ECHO).unwrap(), "run")
            .unwrap();

        // A victim the guest will kill via the ABI.
        let victim = rt.spawn(|_| std::future::pending::<()>());
        let victim_pid = victim.pid();

        // Controlling *other* processes (kill/list/info/is-alive) needs the
        // process-control capability — Trusted grants it.
        let guest = wr.spawn_component_with(&pre, CapabilityProfile::Trusted.capabilities());
        let guest_pid = guest.pid();

        // A native process pings the guest with [its pid][victim pid], then awaits
        // the guest's reply: [guest pid][flags].
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ping_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            let mut msg = ctx.pid().raw().to_le_bytes().to_vec();
            msg.extend_from_slice(&victim_pid.raw().to_le_bytes());
            ping_rt.send(guest_pid, msg);
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        let reply = rx.await.unwrap();
        let reported_pid = u64::from_le_bytes(reply[0..8].try_into().unwrap());
        let flags = reply[8];
        assert_eq!(reported_pid, guest_pid.raw(), "own-pid via the ABI");
        // All seven ops (register, whereis, info, list, is-alive, kill, unregister)
        // succeeded from inside the component.
        assert_eq!(flags, 0b0111_1111, "every actor op should succeed");

        // Observable effects: the guest killed the victim and released the name.
        for _ in 0..200 {
            if !rt.is_alive(victim_pid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            !rt.is_alive(victim_pid),
            "the guest killed the victim via kill"
        );
        assert_eq!(rt.whereis("echo"), None, "the guest released its name");
        guest.join().await;
    }

    /// Run a kv fixture (`wasm`) against `wr` with `caps`, returning the flags byte
    /// it reports. DRY across the kv scenarios below — the raw-ABI `actor-kv` and the
    /// `rusm-rs`-SDK `rs-kv` fixtures share the same `[pid][flags]` protocol.
    async fn run_kv_fixture(wr: &WasmRuntime, rt: &Runtime, caps: Capabilities, wasm: &[u8]) -> u8 {
        let pre = wr
            .prepare_component(&wr.compile_component(wasm).unwrap(), "run")
            .unwrap();
        let guest = wr.spawn_component_with(&pre, caps);
        let guest_pid = guest.pid();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let driver_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            driver_rt.send(guest_pid, ctx.pid().raw().to_le_bytes().to_vec());
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        let reply = rx.await.unwrap();
        guest.join().await;
        reply[8]
    }

    /// A unique store path for a test (distinct per test so parallel runs never
    /// share redb's exclusive file lock); removed first to start from a clean slate.
    fn kv_test_path(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rusm-kv-{tag}-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    const ACTOR_KV: &[u8] = include_bytes!("../../tests/fixtures/actor_kv.wasm");
    const RS_KV: &[u8] = include_bytes!("../../tests/fixtures/rs_kv.wasm");

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_component_uses_kv_when_storage_is_granted() {
        // Storage capability + a configured store → the full CRUD sequence works,
        // both via the raw ABI (actor-kv) and the ergonomic rusm-rs kv module (rs-kv).
        for wasm in [ACTOR_KV, RS_KV] {
            let rt = Runtime::new();
            let path = kv_test_path("granted");
            let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();
            let flags =
                run_kv_fixture(&wr, &rt, CapabilityProfile::Trusted.capabilities(), wasm).await;
            assert_eq!(
                flags, 0b11_1111,
                "every kv op should succeed when storage is granted and a store is configured"
            );
            let _ = std::fs::remove_file(&path);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_go_guest_spawns_dynamic_js_via_spawn_from() {
        // A Go guest (rusm.SpawnFrom) loads runtime JS into the "runner" template with an
        // inline source; the JS runs under the template's profile and messages back —
        // proving the guest-callable spawn-from path end to end (any guest language can
        // drive it; here Go).
        const GUEST: &[u8] = include_bytes!("../../tests/fixtures/go_spawn_from.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_template("runner", CapabilityProfile::Trusted.capabilities());
        let pre = wr
            .prepare_component(&wr.compile_component(GUEST).unwrap(), "run")
            .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.register("collector", collector.pid());

        // The guest needs `spawn` to call SpawnFrom; inline JS needs no extra I/O cap.
        wr.spawn_component_with(&pre, CapabilityProfile::Trusted.capabilities());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ran from go",
            "the Go guest spawned runtime inline JS that ran under the template profile"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_typescript_guest_spawns_dynamic_js_via_spawn_from() {
        // A TS guest calls Process.spawn(name, source) — the spawn-from path — to load an
        // inline JS bundle into the "runner" template; the loaded JS runs under the
        // template profile and messages back. Proves the third language (TS) drives it.
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_template("runner", CapabilityProfile::Trusted.capabilities());

        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const inner = 'module.exports.default = async function () { Process.send(Process.whereis("collector"), "ran from ts"); };';
                Process.spawn("runner", "inline:" + inner);
            };
        "#;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.register("collector", collector.pid());

        let _guest = wr.spawn_js_with(
            BUNDLE.as_bytes().to_vec(),
            CapabilityProfile::Trusted.capabilities(),
        );
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ran from ts",
            "the TS guest spawned runtime inline JS that ran under the template profile"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rust_guest_spawns_dynamic_js_via_spawn_from() {
        // A Rust guest (rusm_rs::spawn_from) loads inline JS into the "runner" template;
        // the loaded JS runs under the template profile and messages back. Completes the
        // set: all three guest languages (Go, TS, Rust) drive spawn-from.
        const GUEST: &[u8] = include_bytes!("../../tests/fixtures/rs_spawn_from.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_template("runner", CapabilityProfile::Trusted.capabilities());
        let pre = wr
            .prepare_component(&wr.compile_component(GUEST).unwrap(), "run")
            .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.register("collector", collector.pid());

        wr.spawn_component_with(&pre, CapabilityProfile::Trusted.capabilities());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ran from rs",
            "the Rust guest spawned runtime inline JS that ran under the template profile"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_from_runs_dynamic_js_under_the_template_profile() {
        // A node declares a JS runner *template* (a capability profile, no fixed bundle);
        // a guest spawns an instance of it with a runtime-chosen source. Here the JS is
        // staged in the node store and spawned via `kv:` — it runs under the template's
        // declared profile and reports back, proving the dynamic-source path end to end.
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let path = kv_test_path("spawn-from");
        let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();
        wr.register_js_template("runner", CapabilityProfile::Trusted.capabilities());

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.register("collector", collector.pid());

        // The spawner (Trusted → spawn + storage): stage the bundle, then spawn-from it.
        const BUNDLE: &[u8] = br#"module.exports.default = async function () {
            Process.send(Process.whereis("collector"), "ran from kv");
        };"#;
        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        host.kv_bucket("bundles")
            .unwrap()
            .set("app", BUNDLE)
            .unwrap();
        host.spawn_from("runner".into(), "kv:bundles/app".into())
            .await
            .unwrap();

        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ran from kv",
            "the dynamic JS, sourced from kv at runtime, ran on the template"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_from_compiles_caches_and_runs_a_dynamic_wasm_component() {
        // The dynamic-WASM twin of the JS template: a node declares a *WASM* runner
        // template (a profile, no fixed bundle); a guest spawns an instance from a
        // runtime-chosen `.wasm` source. The bundle is a real component — compiled on the
        // first spawn (cold) and served from the content-addressed cache thereafter (hot).
        // It runs under the template's profile and answers, proving the whole path. A
        // second spawn of the same source reuses the compile (the cache is exercised, not
        // just the first compile).
        use crate::bindings::rusm::runtime::actor::Host;
        const RS_GUEST: &[u8] = include_bytes!("../../tests/fixtures/rs_guest.wasm");
        let rt = Runtime::new();
        let path = kv_test_path("spawn-from-wasm");
        let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();
        wr.register_wasm_template("runner", CapabilityProfile::Trusted.capabilities());

        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        host.kv_bucket("bundles")
            .unwrap()
            .set("app", RS_GUEST)
            .unwrap();

        // Spawn the dynamic component twice from the same source; each instance answers a
        // collector, so both the cold-compile and the hot-cache-hit paths are proven to run.
        for label in ["cold", "hot"] {
            let pid = host
                .spawn_from("runner".into(), "kv:bundles/app".into())
                .await
                .unwrap_or_else(|e| panic!("{label} spawn-from failed: {e}"));
            let (tx, rx) = tokio::sync::oneshot::channel();
            let collector = rt.spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await.message().unwrap());
            });
            // rs_guest learns its reply-to from the first message, then answers.
            rt.send(
                rusm_otp::Pid::from_raw(pid),
                collector.pid().raw().to_string().into_bytes(),
            );
            assert_eq!(
                String::from_utf8(rx.await.unwrap()).unwrap(),
                format!("hello from {pid}"),
                "the {label} dynamic-WASM instance ran under the template and answered"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_from_fetches_a_url_bundle_via_the_injected_resolver() {
        // A `url:` source is fetched by the node-injected resolver (rusm-wasm stays
        // HTTP-free). The fetched JS runs under the template profile; the fetch is gated
        // by the spawner's own network capability.
        use crate::bindings::rusm::runtime::actor::Host;
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        wr.register_js_template("runner", CapabilityProfile::Trusted.capabilities());

        // Stand-in resolver: serve a known JS bundle for any URL (the real one is reqwest,
        // wired by rusm-cli).
        let resolver: crate::BundleResolver = std::sync::Arc::new(|_url: String| {
            Box::pin(async move {
                Ok(br#"module.exports.default = async function () {
                    Process.send(Process.whereis("collector"), "ran from url");
                };"#
                .to_vec())
            })
        });
        wr.set_bundle_resolver(resolver);

        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.register("collector", collector.pid());

        // A spawn-but-no-network spawner is denied the url fetch (the network gate).
        let mut sandboxed = test_host(&wr, &rt, Capabilities::nothing().allow_spawn(true));
        let denied = sandboxed
            .spawn_from("runner".into(), "url:http://somesite.com/x.js".into())
            .await
            .unwrap_err();
        assert!(
            denied.contains("network"),
            "url fetch needs the network cap: {denied}"
        );

        // A networked spawner fetches + runs it.
        let mut host = test_host(&wr, &rt, CapabilityProfile::Trusted.capabilities());
        host.spawn_from("runner".into(), "url:http://somesite.com/x.js".into())
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "ran from url",
            "the resolver-fetched JS ran on the template"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kv_is_denied_without_the_storage_capability() {
        // A store is configured, but a sandboxed guest lacks the storage grant, so
        // even `kv-set` is refused — no bit is set.
        let rt = Runtime::new();
        let path = kv_test_path("denied");
        let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();
        let flags = run_kv_fixture(
            &wr,
            &rt,
            CapabilityProfile::Sandboxed.capabilities(),
            ACTOR_KV,
        )
        .await;
        assert_eq!(flags, 0, "kv must be denied without the storage capability");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kv_errors_when_no_store_is_configured() {
        // The storage capability is granted, but the runtime was built without a
        // store, so kv ops err (the other arm of the host's gate).
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let flags = run_kv_fixture(
            &wr,
            &rt,
            CapabilityProfile::Trusted.capabilities(),
            ACTOR_KV,
        )
        .await;
        assert_eq!(
            flags, 0,
            "kv must err when no store is configured on the node"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_javascript_bundle_uses_kv_when_granted() {
        // The TS `kv` bridge over the host kv ABI: a full CRUD cycle on the `specs`
        // bucket, reported as a flags integer (63 = all six steps correct).
        const BUNDLE: &str = r#"
            module.exports.default = async function () {
                const replyTo = await Process.receiveText();
                const b = kv.bucket("specs");
                let flags = 0;
                b.set("k", "v1"); flags |= 1;
                const got = b.get("k");
                if (got && new TextDecoder().decode(got) === "v1") flags |= 2;
                if (b.exists("k")) flags |= 4;
                const ks = b.list();
                if (ks.length === 1 && ks[0] === "k") flags |= 8;
                if (b.delete("k")) flags |= 16;
                if (b.get("k") === null) flags |= 32;
                Process.send(replyTo, String(flags));
            };
        "#;
        let rt = Runtime::new();
        let path = kv_test_path("ts-granted");
        let wr = WasmRuntime::with_store(rt.clone(), &path).unwrap();
        let guest = wr.spawn_js_with(BUNDLE.as_bytes(), CapabilityProfile::Trusted.capabilities());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let collector = rt.spawn(move |mut ctx| async move {
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });
        rt.send(guest.pid(), collector.pid().raw().to_string().into_bytes());
        assert_eq!(
            String::from_utf8(rx.await.unwrap()).unwrap(),
            "63",
            "the TS kv bridge performs a full CRUD cycle when storage is granted"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_component_can_receive_with_a_timeout() {
        // A guest that calls `receive-timeout` (Erlang's `receive … after`): once
        // with an empty mailbox (must time out → none) and once after the host
        // sends a message ahead of the deadline (must return it, not drop it).
        const TIMEOUT: &[u8] = include_bytes!("../../tests/fixtures/actor_timeout.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(TIMEOUT).unwrap(), "run")
            .unwrap();
        let guest = wr.spawn_component(&pre); // Sandboxed: receive needs no capability.
        let guest_pid = guest.pid();

        // Drive the fixture's handshake: send our pid, wait for its "armed" signal,
        // then deliver "ping" before its (long) deadline, then collect the report.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let driver_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            driver_rt.send(guest_pid, ctx.pid().raw().to_le_bytes().to_vec());
            assert_eq!(
                ctx.recv().await.message().unwrap(),
                b"armed",
                "fixture arms"
            );
            driver_rt.send(guest_pid, b"ping".to_vec());
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        let reply = rx.await.unwrap();
        assert_eq!(
            u64::from_le_bytes(reply[0..8].try_into().unwrap()),
            guest_pid.raw()
        );
        // bit 0: the empty-mailbox receive timed out; bit 1: the pre-deadline
        // message was returned.
        assert_eq!(
            reply[8], 0b11,
            "receive-timeout must time out when idle and deliver a message before the deadline"
        );
        guest.join().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sandboxed_component_cannot_control_other_processes() {
        // The SAME echo fixture, but spawned Sandboxed (default-deny): it may
        // manage itself (register/whereis/info-self/list-self/unregister) but NOT
        // inspect or kill its neighbours — so is-alive(victim) and kill(victim) are
        // denied, and the victim survives.
        const ECHO: &[u8] = include_bytes!("../../tests/fixtures/actor_echo.wasm");
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(ECHO).unwrap(), "run")
            .unwrap();

        let victim = rt.spawn(|_| std::future::pending::<()>());
        let victim_pid = victim.pid();
        let guest = wr.spawn_component(&pre); // Sandboxed (no process-control)
        let guest_pid = guest.pid();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let ping_rt = rt.clone();
        rt.spawn(move |mut ctx| async move {
            let mut msg = ctx.pid().raw().to_le_bytes().to_vec();
            msg.extend_from_slice(&victim_pid.raw().to_le_bytes());
            ping_rt.send(guest_pid, msg);
            let _ = tx.send(ctx.recv().await.message().unwrap());
        });

        let flags = rx.await.unwrap()[8];
        // Self-ops succeed (register, whereis, info-self, list-contains-self,
        // unregister = bits 0,1,2,3,6); control-of-others denied (is-alive bit 4,
        // kill bit 5 = 0).
        assert_eq!(
            flags, 0b0100_1111,
            "self-ops allowed, control-of-others denied"
        );
        assert!(
            rt.is_alive(victim_pid),
            "the sandboxed guest must NOT be able to kill a neighbour"
        );
        guest.kill();
    }

    // NOTE: the "generous cap *permits* growth" case is tested deterministically as a unit
    // test of the limiter — `the_memory_limiter_enforces_exactly_the_capability_ceiling` in
    // bridges/mod.rs. It is intentionally NOT an integration grow here: a real grow also
    // depends on the OS committing the permitted pages, an environmental factor that made
    // the old `a_generous_memory_cap_lets_a_component_grow` flake under the full suite's
    // memory footprint. RUSM's responsibility (the cap decision) is what we assert; the
    // end-to-end *rejection* path is covered above by
    // `a_memory_cap_crashes_a_component_that_grows_past_it` (no OS commit needed).

    // Deliberately heavy (128 instances committing ~256 MiB) — a scale *proof*, not a
    // routine check. `#[ignore]` keeps its footprint out of the default suite (a real grow
    // depends on OS commit, so at this size it would be environment-sensitive); run it on
    // demand: `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "heavy scale proof; run with --ignored"]
    async fn one_engine_grows_many_instances() {
        // The PROPER guarantee behind the pool: a *single* engine backs memory growth for
        // *many* instances reliably — what a production node does (one engine per process).
        // Run an explicit 256-slot pool and spawn 128 instances that each force a 2 MiB
        // grow, then **report success**; every one must report. (Counting reports, not exit
        // reasons: these exit so fast that a monitor set up afterwards races to `NoProc`.)
        // This proves the engine itself scales — there is no per-engine growth cap.
        const RS_GROW: &[u8] = include_bytes!("../../tests/fixtures/rs_grow.wasm");
        const N: usize = 128;
        let rt = Runtime::new();
        // One engine, 128 slots of 8 MiB each (a 1 GiB reservation — kept modest so this
        // proof doesn't itself add the multi-engine pressure it's characterising). 8 MiB is
        // ample for the grower's ~3.5 MiB footprint.
        let wr = WasmRuntime::with_limits(rt.clone(), N as u32, 8 << 20).unwrap();
        let pre = wr
            .prepare_component(&wr.compile_component(RS_GROW).unwrap(), "run")
            .unwrap();

        // The collector counts "grew" reports; a failed grow would trap before reporting.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let collector = rt.spawn(move |mut ctx| async move {
            loop {
                if let rusm_otp::Received::Message(b) = ctx.recv().await {
                    let _ = tx.send(b);
                }
            }
        });
        rt.register("collector", collector.pid());

        let caps = Capabilities::nothing().max_memory(8 << 20);
        for _ in 0..N {
            wr.spawn_component_with(&pre, caps.clone());
        }
        for i in 0..N {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("only {i}/{N} instances reported a successful grow"))
                .expect("collector channel stays open");
            assert_eq!(msg, b"grew", "an instance grew on the single engine");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_component_runs_as_a_process_and_is_reaped() {
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let component = wr.compile_component(COMP_RUN).unwrap();
        let pre = wr.prepare_component(&component, "run").unwrap();
        wr.spawn_component(&pre).join().await;
        assert_eq!(rt.finished(), 1);
        assert_eq!(rt.process_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_trapping_component_crashes_the_process() {
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let component = wr.compile_component(COMP_TRAP).unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let watcher = rt
            .spawn(move |mut ctx| async move {
                let _ = tx.send(ctx.recv().await);
            })
            .pid();
        let pre = wr.prepare_component(&component, "run").unwrap();
        let guest = wr.spawn_component(&pre);
        let reference = rt.monitor(watcher, guest.pid());
        let guest_pid = guest.pid();

        assert_eq!(
            rx.await.unwrap(),
            rusm_otp::Received::Down {
                reference,
                pid: guest_pid,
                reason: ExitReason::Crashed,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_spinning_component_yields_and_stays_killable() {
        let rt = Runtime::new();
        let wr = WasmRuntime::new(rt.clone()).unwrap();
        let component = wr.compile_component(COMP_SPIN).unwrap();

        // A bystander must still run alongside the spinning component — proof the
        // epoch preempts the component just as it does a core module.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let bystander = rt.spawn(move |_| async move {
            let _ = tx.send(());
        });

        let pre = wr.prepare_component(&component, "run").unwrap();
        let spinner = wr.spawn_component(&pre);
        let spinner_pid = spinner.pid();
        rx.await.unwrap();
        bystander.join().await;

        assert!(rt.is_alive(spinner_pid));
        spinner.kill();
        spinner.join().await;
        assert!(!rt.is_alive(spinner_pid));
    }
}
