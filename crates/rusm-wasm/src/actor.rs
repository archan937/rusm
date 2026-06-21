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

use crate::bridges::conn::{SseEvent, WsOut};
use crate::bridges::WasiHost;

wasmtime::component::bindgen!({
    world: "process",
    path: "wit",
    imports: { default: async },
    exports: { default: async },
});

use rusm::runtime::actor;

/// The per-connection HTTP context (method/path/query/params/headers/addr/subprotocol) a
/// serving bridge attaches to a WebSocket/SSE handler process; re-exported so the bridges
/// and store can name it without the full generated path. Returned by the `connection` op.
pub(crate) use rusm::runtime::actor::ConnectionInfo;

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

    /// This process's connection context, set by the serving bridge when it spawned a
    /// per-connection WebSocket/SSE handler; `None` for every other process. A plain
    /// read of per-instance store state — no `rusm-otp` call, no capability gate.
    async fn connection(&mut self) -> Option<actor::ConnectionInfo> {
        self.connection.clone()
    }

    /// Send a text WebSocket frame to this connection's writer (binary frames use the
    /// plain `send` path). `false` if this process is not a WebSocket handler, or the
    /// bounded channel is closed (the client disconnected). The bound back-pressures a
    /// handler that outruns a slow client — it parks on `send` rather than buffering.
    async fn ws_send_text(&mut self, payload: Vec<u8>) -> bool {
        match &self.ws_out {
            Some(tx) => tx.send(WsOut::Text(payload)).await.is_ok(),
            None => false,
        }
    }

    /// Close this WebSocket connection with a status `code` + `reason` (a server-initiated
    /// close frame), then the connection tears down. No-op for a non-WebSocket process.
    async fn ws_close(&mut self, code: u16, reason: String) {
        if let Some(tx) = &self.ws_out {
            let _ = tx.send(WsOut::Close(code, reason)).await;
        }
    }

    /// Emit a rich SSE event (data + optional event/id/retry) to this connection's writer
    /// (the plain `data:` path is a `send` to the writer pid). `false` if this is not an SSE
    /// handler, or the bounded channel is closed (the client disconnected).
    async fn sse_send(
        &mut self,
        data: Vec<u8>,
        event: Option<String>,
        id: Option<String>,
        retry: Option<u32>,
    ) -> bool {
        match &self.sse_out {
            Some(tx) => tx
                .send(SseEvent {
                    data,
                    event,
                    id,
                    retry,
                })
                .await
                .is_ok(),
            None => false,
        }
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
        if entry.dynamic {
            return Err(format!(
                "`{component}` is a dynamic JS template — spawn it with spawn-from(component, source)"
            ));
        }
        // A node-registered component runs under its **declared** profile (the manifest's
        // explicit per-component policy — what's declared is what runs); an ad-hoc
        // registration with no declared profile inherits this process's caps
        // (non-escalating). Either way the `spawn` capability above gates who may spawn.
        let caps = entry.caps.clone().unwrap_or_else(|| self.caps.clone());
        let child = spawner.spawn_component(&entry.prepared, caps, Some(&component));
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
        let bundle = match self.resolve_local(&source)? {
            Some(bytes) => bytes,
            None => {
                // `url:` source: gate on the spawner's own network capability, then fetch
                // via the node-injected resolver. Extract the resolver (an owned Arc) first
                // so no `&self` borrow is held across the await (keeps the future `Send`).
                if !self.caps.network_allowed() {
                    return Err("spawn-from url denied: missing the network capability".to_string());
                }
                let resolver = self
                    .spawner
                    .as_ref()
                    .and_then(|s| s.bundle_resolver.get().cloned())
                    .ok_or("url: bundle sources are not configured on this node")?;
                let url = source
                    .trim()
                    .strip_prefix("url:")
                    .unwrap_or(source.trim())
                    .to_string();
                resolver(url).await?
            }
        };
        let spawner = self.spawner.as_ref().ok_or("spawn unavailable here")?;
        let entry = spawner
            .lookup(&component)
            .ok_or_else(|| format!("unknown component `{component}`"))?;
        if !entry.dynamic {
            return Err(format!(
                "`{component}` is not a dynamic JS template (use spawn for a fixed component)"
            ));
        }
        let caps = entry.caps.clone().unwrap_or_else(|| self.caps.clone());
        let child = spawner.spawn_component(&entry.prepared, caps, Some(&component));
        // The js-runner takes its bundle as message 1 (the runner's protocol).
        self.rt.send(Pid::from_raw(child.pid().raw()), bundle);
        Ok(child.pid().raw())
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
            let prepared = entry.prepared.clone();
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

/// The wire name for an exit reason carried in a `__down` message.
fn down_reason(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::Normal => "normal",
        ExitReason::Killed => "killed",
        ExitReason::Crashed => "crashed",
        ExitReason::NoProc => "noproc",
    }
}
