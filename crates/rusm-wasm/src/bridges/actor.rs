// Canonical source: bridges/actor/host.rs — the actor bridge's native host impl (the
// Erlang Process core). Synced into rusm-wasm (crates/rusm-wasm/src/bridges/actor.rs) by
// `make sync-bridges`; edit this file, not the copy. `bridge_host_in_sync` guards drift.

//! The **actor host ABI**: binds the `rusm:runtime` WIT world and implements its
//! `actor` interface on [`WasiHost`] as thin calls into `rusm-otp`.
//!
//! This is the lift→call-OTP→lower glue — a guest's `send`/`receive`/`list`/`kill`
//! become `rusm-otp` operations. `receive` is **async**, so a guest blocked on its
//! mailbox suspends its fiber and frees the Tokio worker (the "write blocking code,
//! get async" property). The runtime stays the source of truth; this never
//! reimplements OTP.

use std::sync::Arc;
use std::time::Duration;

use rusm_otp::{Context, ExitReason, Pid, Received, Runtime, Strategy};

use crate::bridges::WasiHost;
use crate::DynamicKind;

use crate::bindings::rusm::runtime::actor;

/// Wires the actor interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    actor::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl actor::Host for WasiHost {
    async fn own_pid(&mut self) -> u64 {
        self.pid
    }

    /// Spawn a registered component by name as a new process — the actor model's
    /// `spawn`, the unlock for per-request workers and concealed typed clients.
    /// Capability-gated (`allow-spawn`, default-deny) and **non-escalating**: the
    /// child inherits *this* process's capabilities, so it is never more privileged
    /// than its parent. Errors (rather than trapping) on a denied or unknown spawn.
    async fn spawn(&mut self, component: String) -> Result<u64, String> {
        if !self.caps.can_spawn() {
            return Err("spawn denied: missing the spawn capability".to_string());
        }
        let spawner = self.spawner.as_ref().ok_or("spawn unavailable here")?;
        let entry = spawner
            .lookup(&component)
            .ok_or_else(|| format!("unknown component `{component}`"))?;
        if entry.dynamic.is_some() {
            return Err(format!(
                "`{component}` is a dynamic runner template — spawn it with spawn-from(component, source)"
            ));
        }
        // A node-registered component runs under its **declared** profile (the manifest's
        // explicit per-component policy — what's declared is what runs); an ad-hoc
        // registration with no declared profile inherits this process's caps
        // (non-escalating). Either way the `spawn` capability above gates who may spawn.
        let caps = entry.caps.clone().unwrap_or_else(|| self.caps.clone());
        let prepared = entry
            .prepared
            .as_ref()
            .ok_or_else(|| format!("`{component}` has no prepared component"))?;
        let child = spawner.spawn_component(prepared, caps, Some(&component));
        // A TS service carries its bundle as message 1 (the js-runner's protocol).
        if let Some(bundle) = &entry.bundle {
            self.rt
                .send(Pid::from_raw(child.pid().raw()), (**bundle).clone());
        }
        Ok(child.pid().raw())
    }

    /// Spawn a **dynamic JS** instance of a registered runner template, with the JS bundle
    /// supplied at runtime by `source`. The loaded JS runs under the template's *declared*
    /// profile (operator policy — the guest picks the code, never the capabilities).
    /// `source` is `inline:<js>` (the bundle itself), `kv:<bucket>/<key>` (the node store),
    /// or `url:`/`http(s)://…` (fetched). Capability-gated: `spawn` always, plus the
    /// spawner's own `storage` (kv) / `network` (url); `inline` needs no extra I/O cap.
    async fn spawn_from(&mut self, component: String, source: String) -> Result<u64, String> {
        if !self.caps.can_spawn() {
            return Err("spawn denied: missing the spawn capability".to_string());
        }
        // Clone the spawner handle so no `&self` borrow is held across an await (keeps the
        // future `Send`), then dispatch on the template kind.
        let spawner = self.spawner.clone().ok_or("spawn unavailable here")?;
        let entry = spawner
            .lookup(&component)
            .ok_or_else(|| format!("unknown component `{component}`"))?;
        let caps = entry.caps.clone().unwrap_or_else(|| self.caps.clone());
        match entry.dynamic {
            None => Err(format!(
                "`{component}` is not a dynamic template (use `spawn` for a fixed component)"
            )),
            // Dynamic JS: fetch the bundle and run it on the shared js-runner (message 1).
            Some(DynamicKind::Js) => {
                // Resolve inline:/kv: synchronously; for url:, gate `network` and extract the
                // resolver (an owned `Arc`) *before* awaiting, so no `&self` (the `!Sync`
                // WasiHost) is held across the await — keeping the spawn-from future `Send`.
                let bundle = match self.resolve_local(&source)? {
                    Some(bytes) => bytes,
                    None => {
                        if !self.caps.network_allowed() {
                            return Err(
                                "spawn-from url denied: missing the network capability".to_string()
                            );
                        }
                        let resolver = spawner
                            .bundle_resolver
                            .get()
                            .cloned()
                            .ok_or("url: bundle sources are not configured on this node")?;
                        let url = source
                            .trim()
                            .strip_prefix("url:")
                            .unwrap_or(source.trim())
                            .to_string();
                        resolver(url).await?
                    }
                };
                let prepared = entry
                    .prepared
                    .as_ref()
                    .ok_or("dynamic JS template has no runner")?;
                let child = spawner.spawn_component(prepared, caps, Some(&component));
                // The js-runner takes its bundle as message 1 (the runner's protocol).
                self.rt.send(Pid::from_raw(child.pid().raw()), bundle);
                Ok(child.pid().raw())
            }
            // Dynamic WASM: gate the I/O capability by scheme, then compile (cold once, cached
            // by content hash) and spawn the prepared component on the pooled fast path (hot).
            // `dynamic_wasm` borrows only the `Sync` spawner across its await, so the future
            // stays `Send`.
            Some(DynamicKind::Wasm) => {
                self.gate_source(&source)?;
                let prepared = spawner.dynamic_wasm(&source).await?;
                let child = spawner.spawn_component(prepared.as_ref(), caps, Some(&component));
                Ok(child.pid().raw())
            }
        }
    }

    /// Monitor `target`: when it dies, this process receives a `__down` message
    /// (Erlang's `monitor` — the basis for a guest `Supervisor`). Capability-gated
    /// like spawn (supervisors pair spawn + monitor). No watcher process and no
    /// polling: the runtime's monitor delivers the `Down`, which `receive`
    /// translates — event-driven and cheap.
    async fn monitor(&mut self, target: u64) {
        if self.caps.can_spawn() || self.caps.process_control() {
            self.rt
                .monitor(Pid::from_raw(self.pid), Pid::from_raw(target));
        }
    }

    async fn send(&mut self, to: u64, message: Vec<u8>) {
        self.rt.send(Pid::from_raw(to), message);
    }

    async fn receive(&mut self) -> Vec<u8> {
        let ctx = self
            .ctx
            .as_mut()
            .expect("receive runs inside a spawned process");
        next_message(ctx).await
    }

    /// Erlang's `receive … after`: the next message, or `none` if `timeout_ms`
    /// elapses first. Built on `tokio::time::timeout` over the *same* receive loop
    /// as [`receive`] — `ctx.recv()` is cancel-safe (a dropped await leaves the
    /// mailbox untouched), so a timeout never loses a message. The basis for SSE
    /// heartbeats and any guest-side deadline without a busy poll.
    async fn receive_timeout(&mut self, timeout_ms: u64) -> Option<Vec<u8>> {
        let ctx = self
            .ctx
            .as_mut()
            .expect("receive-timeout runs inside a spawned process");
        tokio::time::timeout(Duration::from_millis(timeout_ms), next_message(ctx))
            .await
            .ok()
    }

    async fn list_processes(&mut self) -> Vec<u64> {
        // Default-deny: without process-control a guest sees only itself.
        if !self.caps.process_control() {
            return vec![self.pid];
        }
        self.rt.list().into_iter().map(|p| p.raw()).collect()
    }

    async fn info(&mut self, target: u64) -> Option<actor::ProcessInfo> {
        if !self.caps.process_control() && target != self.pid {
            return None; // may inspect only itself
        }
        self.rt
            .info(Pid::from_raw(target))
            .map(|i| actor::ProcessInfo {
                pid: i.pid.raw(),
                links: i.links as u32,
                monitors: i.monitors as u32,
                names: i.names,
                label: i.label,
                mailbox_depth: i.mailbox_depth as u32,
                trap_exit: i.trap_exit,
            })
    }

    async fn is_alive(&mut self, target: u64) -> bool {
        if !self.caps.process_control() && target != self.pid {
            return false; // may probe only itself
        }
        self.rt.is_alive(Pid::from_raw(target))
    }

    async fn kill(&mut self, target: u64) -> bool {
        if !self.caps.process_control() && target != self.pid {
            return false; // may terminate only itself
        }
        self.rt.kill(Pid::from_raw(target))
    }

    async fn register(&mut self, name: String) -> bool {
        self.rt.register(name, Pid::from_raw(self.pid))
    }

    async fn whereis(&mut self, name: String) -> Option<u64> {
        self.rt.whereis(&name).map(|p| p.raw())
    }

    async fn unregister(&mut self, name: String) -> bool {
        self.rt.unregister(&name)
    }

    async fn set_label(&mut self, label: String) {
        self.rt.set_label(Pid::from_raw(self.pid), label);
    }

    /// Supervise named child components under the **native** `rusm-otp` supervisor —
    /// the single restart implementation the guest `Supervisor` SDKs delegate to.
    /// Capability-gated like `spawn`; each child is spawned with *this* process's
    /// capabilities (non-escalating). The supervisor is **linked to the caller** and
    /// **traps exits**: if it gives up (restart budget exceeded) the caller dies too,
    /// and if the caller dies the supervisor tears its children down — clean
    /// co-termination, no orphans.
    async fn supervise(
        &mut self,
        strategy: actor::SuperviseStrategy,
        children: Vec<String>,
        max_restarts: u32,
        within_ms: u32,
    ) -> Result<u64, String> {
        if !self.caps.can_spawn() {
            return Err("supervise denied: missing the spawn capability".to_string());
        }
        let spawner = self.spawner.as_ref().ok_or("supervise unavailable here")?;
        let strategy = match strategy {
            actor::SuperviseStrategy::OneForOne => Strategy::OneForOne,
            actor::SuperviseStrategy::OneForAll => Strategy::OneForAll,
            actor::SuperviseStrategy::RestForOne => Strategy::RestForOne,
        };
        let mut sup = self.rt.supervisor(strategy).max_restarts(max_restarts);
        sup = if within_ms == 0 {
            sup.over_lifetime()
        } else {
            sup.within(Duration::from_millis(within_ms as u64))
        };
        for name in children {
            let entry = spawner
                .lookup(&name)
                .ok_or_else(|| format!("unknown component `{name}`"))?;
            // A supervised child is a fixed component (a dynamic template runs only via
            // spawn-from, so it can't be a supervisor child).
            let prepared = entry.prepared.clone().ok_or_else(|| {
                format!("`{name}` is a dynamic template and can't be a supervised child")
            })?;
            let bundle = entry.bundle.clone();
            // A supervised child runs under its OWN declared profile — consistent with a
            // direct spawn-by-name — falling back to the supervisor's caps for an ad-hoc
            // (un-declared) registration.
            let caps = entry.caps.clone().unwrap_or_else(|| self.caps.clone());
            let label = name.clone();
            let spawner = Arc::clone(spawner);
            sup = sup.child(move |rt: &Runtime| {
                let child = spawner.spawn_component(&prepared, caps.clone(), Some(&label));
                if let Some(bundle) = &bundle {
                    rt.send(Pid::from_raw(child.pid().raw()), (**bundle).clone());
                }
                child
            });
        }
        let sup_pid = Pid::from_raw(sup.start().pid().raw());
        // Co-terminate with the caller (see the doc comment).
        self.rt.set_trap_exit(sup_pid, true);
        self.rt.link(Pid::from_raw(self.pid), sup_pid);
        Ok(sup_pid.raw())
    }

    /// Schedule `message` to be delivered to `to` after `delay_ms` milliseconds —
    /// Erlang's `erlang:send_after/3`. Delegates entirely to the OTP primitive; the
    /// bridge only wraps the delay unit and assigns a per-process handle so the guest
    /// can cancel. If `to` is already gone when the timer fires, the delivery is a
    /// silent no-op (like a direct `send`).
    async fn send_after(&mut self, to: u64, delay_ms: u64, message: Vec<u8>) -> u64 {
        let timer = self
            .rt
            .send_after(Pid::from_raw(to), Duration::from_millis(delay_ms), message);
        let id = self.next_timer;
        self.next_timer = self.next_timer.wrapping_add(1);
        self.timers.insert(id, timer);
        id
    }

    /// Cancel a pending timer by handle. Returns `true` if the timer was found and
    /// aborted before it fired; `false` if the handle is unknown — already fired,
    /// already cancelled, or never issued by this process.
    async fn cancel_timer(&mut self, timer_ref: u64) -> bool {
        if let Some(timer) = self.timers.remove(&timer_ref) {
            timer.cancel();
            true
        } else {
            false
        }
    }
}

impl WasiHost {
    /// Resolve the named bucket of the node's store, enforcing the **storage**
    /// capability (default-deny) and that a store is actually configured. Shared by the
    /// `kv` bridge ([`crate::bridges::kv`]) and `spawn-from`'s `kv:` source loader, so the
    /// gate lives in exactly one place.
    pub(crate) fn kv_bucket(&self, bucket: &str) -> Result<rusm_kv::Bucket, String> {
        if !self.caps.storage_allowed() {
            return Err("kv denied: missing the storage capability".to_string());
        }
        let store = self
            .spawner
            .as_ref()
            .and_then(|s| s.store.as_ref())
            .ok_or("kv unavailable: no store configured on this node")?;
        Ok(store.bucket(bucket))
    }

    /// Gate a dynamic-WASM `source` by its scheme **without fetching** — so the capability is
    /// enforced on every spawn (cold *and* hot), and a cached `url:`/`kv:` bundle can never be
    /// reached by a guest lacking `network`/`storage`. `inline:` needs no extra capability.
    fn gate_source(&self, source: &str) -> Result<(), String> {
        let source = source.trim();
        if source.starts_with("kv:") && !self.caps.storage_allowed() {
            return Err("spawn-from kv denied: missing the storage capability".to_string());
        }
        let is_url = source.starts_with("url:")
            || source.starts_with("http://")
            || source.starts_with("https://");
        if is_url && !self.caps.network_allowed() {
            return Err("spawn-from url denied: missing the network capability".to_string());
        }
        Ok(())
    }

    /// Resolve a `spawn-from` source that needs no network fetch — `inline:<js>` (the
    /// bundle verbatim) or `kv:<bucket>/<key>` (the node store, enforcing `storage` via
    /// `kv_bucket`). `Ok(None)` signals a `url:`/`http(s)://` source, which `spawn-from`
    /// fetches via the node-injected resolver (enforcing `network`); `Err` is an
    /// unrecognised source.
    fn resolve_local(&self, source: &str) -> Result<Option<Vec<u8>>, String> {
        let source = source.trim();
        if let Some(js) = source.strip_prefix("inline:") {
            return Ok(Some(js.as_bytes().to_vec()));
        }
        if let Some(rest) = source.strip_prefix("kv:") {
            let (bucket, key) = rest
                .split_once('/')
                .ok_or_else(|| format!("kv source must be `kv:<bucket>/<key>`, got {source:?}"))?;
            let bytes = self
                .kv_bucket(bucket)?
                .get(key)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("bundle not found at {source}"))?;
            return Ok(Some(bytes));
        }
        let url = source.strip_prefix("url:").unwrap_or(source);
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(None); // fetched by spawn-from via the injected resolver
        }
        Err(format!(
            "unrecognised bundle source {source:?} \
             (expected `inline:<js>`, `kv:<bucket>/<key>`, or `http(s)://…`)"
        ))
    }
}

/// The shared receive loop behind `receive` and `receive-timeout`: return the next
/// *user-visible* mailbox item as message bytes — a plain message verbatim, or a
/// monitored `Down` rendered as a `__down` JSON message (Erlang delivers Down to
/// the mailbox, the basis for a guest `Supervisor`). Streams and other signals are
/// skipped. Kept free-standing so both callers borrow `ctx` and share one body.
async fn next_message(ctx: &mut Context) -> Vec<u8> {
    loop {
        match ctx.recv().await {
            Received::Message(bytes) => return bytes,
            Received::Down { pid, reason, .. } => {
                let reason = down_reason(reason);
                return format!(r#"{{"__down":"{}","reason":"{reason}"}}"#, pid.raw()).into_bytes();
            }
            _ => {} // streams / other signals are skipped here
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bindings::rusm::runtime::actor::Host;
    use crate::bridges::{HttpCaps, WasiHost};
    use crate::caps::Capabilities;
    use rusm_otp::Runtime;
    use std::collections::HashMap;
    use wasmtime_wasi::ResourceTable;
    use wasmtime_wasi::WasiCtxBuilder;
    use wasmtime_wasi_http::WasiHttpCtx;

    fn timer_host() -> WasiHost {
        WasiHost {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            http: WasiHttpCtx::new(),
            http_hooks: HttpCaps {
                allow_network: false,
            },
            pid: 0,
            connection: None,
            ws_out: None,
            sse_out: None,
            caps: Capabilities::nothing(),
            rt: Runtime::new(),
            ctx: None,
            spawner: None,
            out_streams: HashMap::new(),
            in_streams: HashMap::new(),
            next_stream: 0,
            timers: HashMap::new(),
            next_timer: 0,
        }
    }

    #[tokio::test]
    async fn send_after_assigns_sequential_handles() {
        // Each `send-after` call gets a monotonically-increasing per-process handle,
        // and every pending timer is tracked in the map.
        let mut host = timer_host();
        let h0 = host.send_after(0, 60_000, b"a".to_vec()).await;
        let h1 = host.send_after(0, 60_000, b"b".to_vec()).await;
        assert_eq!(h0, 0, "first handle is 0");
        assert_eq!(h1, 1, "second handle is 1");
        assert_eq!(host.timers.len(), 2, "both timers are pending");
    }

    #[tokio::test]
    async fn cancel_timer_removes_the_handle_and_returns_true() {
        let mut host = timer_host();
        let handle = host.send_after(0, 60_000, b"x".to_vec()).await;
        assert!(host.cancel_timer(handle).await, "cancel returns true");
        assert!(
            !host.timers.contains_key(&handle),
            "handle removed from map after cancel"
        );
    }

    #[tokio::test]
    async fn cancel_timer_returns_false_for_unknown_ref() {
        let mut host = timer_host();
        assert!(!host.cancel_timer(99).await, "unknown ref returns false");
    }

    #[tokio::test]
    async fn double_cancel_returns_false_on_second_call() {
        let mut host = timer_host();
        let handle = host.send_after(0, 60_000, b"x".to_vec()).await;
        assert!(host.cancel_timer(handle).await);
        assert!(!host.cancel_timer(handle).await, "already removed → false");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_after_delivers_to_target_process() {
        // A short delay (5 ms) keeps the test fast. Delivery semantics are the OTP
        // primitive's responsibility — tested exhaustively in rusm-otp; here we only
        // verify the bridge wires them end-to-end.
        let mut host = timer_host();
        let rt = host.rt.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
        let target = rt.spawn(move |mut ctx| async move {
            let msg = ctx.recv().await.message().unwrap();
            let _ = tx.send(msg);
        });
        host.send_after(target.pid().raw(), 5, b"ding".to_vec())
            .await;
        assert_eq!(rx.await.unwrap(), b"ding".to_vec());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_timer_never_delivers() {
        // Cancel immediately after scheduling (60 s delay — won't fire during the
        // test). Yield briefly and verify nothing landed in the mailbox.
        let mut host = timer_host();
        let rt = host.rt.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let target = rt.spawn(move |mut ctx| async move {
            loop {
                if let Some(msg) = ctx.recv().await.message() {
                    let _ = tx.send(msg);
                }
            }
        });
        let handle = host
            .send_after(target.pid().raw(), 60_000, b"x".to_vec())
            .await;
        assert!(host.cancel_timer(handle).await);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err(), "cancelled timer must not deliver");
        target.kill();
    }
}

/// The wire name for an exit reason carried in a `__down` message.
fn down_reason(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::Normal => "normal",
        ExitReason::Killed => "killed",
        ExitReason::Crashed => "crashed",
        ExitReason::NoProc => "noproc",
    }
}
