use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use futures_util::future::{AbortHandle, Abortable};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::exit::{ExitReason, MonitorRef};
use crate::message::{Envelope, Message, Meta, Received};
use crate::pid::Pid;
use crate::stream::StreamHandle;

/// What a process body receives when it starts: its own [`Pid`] and its
/// **mailbox** — the receiving end of its message queue.
pub struct Context {
    pid: Pid,
    mailbox: UnboundedReceiver<Envelope>,
    /// Items pulled from the channel but skipped over by a selective
    /// [`recv_match`](Context::recv_match), kept in arrival order. A later
    /// receive sees them before anything still in the channel — the Erlang
    /// "save queue". Empty (and allocation-free) unless selective receive is used.
    saved: VecDeque<Envelope>,
    /// Optional mailbox-depth counter (decrement side). `None` unless the runtime
    /// was built with [`Runtime::with_mailbox_depth`] — so the default hot path
    /// pays no allocation and no atomic.
    depth: Option<Arc<AtomicUsize>>,
    /// The opaque [`Meta`] that rode with the most-recently-received item, refreshed
    /// by every [`recv`](Self::recv)/[`recv_match`](Self::recv_match). `None` after a
    /// plain send or a system signal. A host layer reads it via [`current_meta`](Self::current_meta).
    current_meta: Meta,
    /// Items a caller **stashed** ([`stash`](Self::stash)) while awaiting a specific reply,
    /// each keeping its own [`Meta`]. Held aside (never re-read by `recv` mid-wait), then
    /// [`unstash`](Self::unstash)ed to the front of `saved` so a later `recv` re-delivers
    /// them — meta intact. This is what lets a host layer do selective receive **without
    /// losing per-message metadata** on replay (the guest cannot carry host-only meta itself).
    /// Empty (and allocation-free) unless a guest does an RPC-style selective receive.
    deferred: VecDeque<Envelope>,
}

impl Context {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Records that one item left the mailbox (no-op unless depth is tracked).
    fn note_consumed(&self) {
        if let Some(depth) = &self.depth {
            depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Receives the next item, suspending the process until one arrives (FIFO),
    /// exactly like an Erlang `receive`. The result is usually a user
    /// [`Received::Message`], but a process that monitors or trap-links others
    /// also gets [`Received::Down`]/[`Received::Exit`] here, in arrival order. A
    /// process blocked here parks with zero cost until something arrives or a
    /// [`kill`](Runtime::kill) wakes it.
    pub async fn recv(&mut self) -> Received {
        let envelope = match self.saved.pop_front() {
            Some(envelope) => envelope,
            None => self.next_from_mailbox().await,
        };
        self.note_consumed();
        self.current_meta = envelope.meta;
        envelope.received
    }

    /// The opaque [`Meta`] that accompanied the item the last [`recv`](Self::recv)/
    /// [`recv_match`](Self::recv_match) returned, or `None` (a plain send, a system
    /// signal, or before any receive). The runtime never interprets it — a host layer
    /// downcasts the `Arc<dyn Any>` to its own type. Cheap to call (an `Arc` clone).
    pub fn current_meta(&self) -> Meta {
        self.current_meta.clone()
    }

    /// **Set aside** the message just received, keeping its [`Meta`], while continuing to
    /// wait for a different one — the host primitive behind a guest SDK's RPC selective
    /// receive (await a reply, hold unrelated mail). The stashed item is **not** re-read by
    /// the intervening [`recv`](Self::recv)s; [`unstash`](Self::unstash) returns it (and any
    /// siblings, in stash order) to the front of the queue for a later `recv`, which rebinds
    /// its meta. Stashing here — not guest-side — is what keeps per-message metadata correct
    /// across the deferral (a guest can't carry host-only meta). `message` is the bytes the
    /// caller already received; its meta is the current one ([`current_meta`](Self::current_meta)).
    pub fn stash(&mut self, message: Message) {
        // Re-queued: it was counted consumed when `recv` returned it; balance that so the
        // eventual `unstash` → `recv` decrement nets out (no-op unless depth is tracked).
        if let Some(depth) = &self.depth {
            depth.fetch_add(1, Ordering::Relaxed);
        }
        self.deferred.push_back(Envelope {
            received: Received::Message(message),
            meta: self.current_meta.clone(),
        });
    }

    /// Return every [`stash`](Self::stash)ed item to the **front** of the queue, in stash
    /// (arrival) order, so the next [`recv`](Self::recv)s re-deliver them — each rebinding its
    /// own meta — before any newer mail. Called once the awaited reply has arrived. A no-op
    /// when nothing was stashed.
    pub fn unstash(&mut self) {
        while let Some(envelope) = self.deferred.pop_back() {
            self.saved.push_front(envelope);
        }
    }

    /// Receives the next item for which `matches` is true, suspending until one
    /// arrives. Items that don't match are left queued in arrival order for a
    /// later receive — Erlang's selective `receive`. Already-saved items are
    /// considered first, so this never reorders the mailbox.
    pub async fn recv_match<F>(&mut self, mut matches: F) -> Received
    where
        F: FnMut(&Received) -> bool,
    {
        if let Some(pos) = self.saved.iter().position(|e| matches(&e.received)) {
            let envelope = self.saved.remove(pos).expect("position is in bounds");
            self.note_consumed();
            self.current_meta = envelope.meta;
            return envelope.received;
        }
        loop {
            let envelope = self.next_from_mailbox().await;
            if matches(&envelope.received) {
                self.note_consumed();
                self.current_meta = envelope.meta;
                return envelope.received;
            }
            self.saved.push_back(envelope);
        }
    }

    async fn next_from_mailbox(&mut self) -> Envelope {
        // The sole sender lives in the process table, which the running task
        // keeps alive through its own `Arc<Inner>`; it is removed only after this
        // body returns. So while we are awaiting here the channel cannot close —
        // a live process always has a message coming or is parked forever.
        self.mailbox
            .recv()
            .await
            .expect("a live process always holds its own mailbox sender")
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The mailbox receiver isn't meaningfully printable; the pid identifies it.
        f.debug_struct("Context").field("pid", &self.pid).finish()
    }
}

/// A handle to a spawned process: address it ([`kill`](ProcessHandle::kill)) and
/// await it ([`join`](ProcessHandle::join)).
pub struct ProcessHandle {
    pid: Pid,
    abort: AbortHandle,
    join: JoinHandle<()>,
}

impl ProcessHandle {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Stops the process at its next suspension point. Cleanup still runs (the
    /// table entry is removed and the process counted finished), because that
    /// lives on the body's drop path.
    pub fn kill(&self) {
        self.abort.abort();
    }

    /// Waits for the process to terminate (ignores a body panic or a kill).
    pub async fn join(self) {
        let _ = self.join.await;
    }
}

/// A point-in-time snapshot of a live process for observability — the analogue
/// of Erlang's `Process.info/1`. Cheap to produce (a single table lookup). Run
/// vs. suspended *status* is deliberately omitted: Tokio doesn't expose a task's
/// park state, and faking it would mislead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: Pid,
    /// Number of bidirectionally linked peers.
    pub links: usize,
    /// Number of processes monitoring this one.
    pub monitors: usize,
    /// Registry names this process holds.
    pub names: Vec<String>,
    /// The optional human-readable label (see [`Runtime::set_label`]).
    pub label: Option<String>,
    /// Items waiting in the mailbox (channel + save queue), not yet consumed.
    pub mailbox_depth: usize,
    /// Whether this process traps exits.
    pub trap_exit: bool,
}

/// One process this entry records is monitoring us; on our exit it gets a
/// [`Received::Down`] tagged with `reference`.
struct Monitor {
    watcher: Pid,
    reference: MonitorRef,
}

/// What the runtime keeps for each live process. A process needs **only one
/// channel** — the mailbox; exit signals ride it as [`Received`], and kill rides
/// a `futures` abort handle (which exists *before* the task is spawned, so the
/// whole entry is written in a single race-free insert). Erlang runtimes and
/// Lunatic keep a *second*, signal channel per process; we don't.
///
/// `links`, `monitors` and `exit_reason` are empty/false/`None` for an ordinary
/// process and cost no allocation — only fault-tolerant processes pay for them.
struct ProcessEntry {
    abort: AbortHandle,
    mailbox: UnboundedSender<Envelope>,
    /// When set, incoming exit signals arrive as [`Received::Exit`] messages
    /// instead of killing this process (Erlang's `process_flag(trap_exit, true)`).
    trap_exit: bool,
    /// Bidirectionally linked peers — each also lists us.
    links: Vec<Pid>,
    /// Processes monitoring us.
    monitors: Vec<Monitor>,
    /// Names this process holds in the registry, released on exit.
    names: Vec<String>,
    /// Process-group tags this process holds (Erlang `pg`-style: one pid → many tags,
    /// one tag → many pids), released on exit alongside `names`. Empty (zero-allocation)
    /// until the process tags itself, so an untagged process costs nothing here.
    tags: Vec<String>,
    /// A reason staged by a link cascade, so this process exits with the
    /// *original* reason rather than the bare `Killed` an abort would imply.
    exit_reason: Option<ExitReason>,
    /// An optional human-readable label for observability (Elixir's
    /// `Process.set_label`), distinct from a registered name. `None` and
    /// allocation-free until set.
    label: Option<String>,
    /// Optional mailbox-depth counter (increment side). `None` unless the runtime
    /// tracks depth (see [`Runtime::with_mailbox_depth`]); shared with the
    /// [`Context`] receive side and read by [`ProcessInfo`].
    depth: Option<Arc<AtomicUsize>>,
}

impl ProcessEntry {
    /// Records that one item entered the mailbox (no-op unless depth is tracked).
    fn note_enqueued(&self) {
        if let Some(depth) = &self.depth {
            depth.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Current mailbox depth (0 unless depth is tracked — which it always is when a
    /// capacity is set, since capacity is enforced against this count).
    fn depth_value(&self) -> usize {
        self.depth.as_ref().map_or(0, |d| d.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
struct Inner {
    /// Whether to track per-process mailbox depth (off by default). Off means a
    /// spawn allocates no counter and send/recv do no atomics — see
    /// [`Runtime::with_mailbox_depth`].
    track_depth: bool,
    /// Opt-in per-process mailbox capacity (off by default → unbounded). When set,
    /// `enqueue` sheds *user* messages once a mailbox holds this many — overload
    /// protection. System signals are never shed. See
    /// [`Runtime::with_mailbox_capacity`].
    mailbox_capacity: Option<usize>,
    /// User messages shed because a bounded mailbox was at capacity.
    dropped: AtomicU64,
    /// **Platform lifecycle log** verbosity (a [`LogLevel`] rank; `0` = `Off`, the
    /// default → zero hot-path cost). When above `Off`, a labeled process's spawn /
    /// exit / restart is logged to stderr at its event level. Set via
    /// [`Runtime::set_log_level`].
    log_level: std::sync::atomic::AtomicU8,
    next_id: AtomicU64,
    next_ref: AtomicU64,
    spawned: AtomicU64,
    finished: AtomicU64,
    // Sharded concurrent map: spawners and completers mostly touch different
    // shards, so the process table isn't a global-lock bottleneck under a storm.
    table: DashMap<u64, ProcessEntry>,
    // name -> pid. Sharded too, so name lookups never take a global lock the way
    // Lunatic's single `RwLock<HashMap>` registry does.
    registry: DashMap<String, u64>,
    // tag -> live pids: Erlang `pg`-style process groups (one pid → many tags, one tag →
    // many pids). Sharded like the registry; touched only by register/unregister/whereis/
    // kill_tag and the exit reaper — never by spawn or message passing, so the hot path is
    // unaffected. Members are removed on exit (see `deregister`).
    tags: DashMap<String, HashSet<u64>>,
    /// Bumped on each labeled-process state change (a spawn gets named, or a labeled
    /// process exits) **only when logging is at `Info`+** — the census task reads it to
    /// tell "something happened since my last line" from "nothing changed". This is what
    /// distinguishes a real spawn+exit (gen advanced twice, net-same counts → *still*
    /// logged) from a quiet node (gen unchanged → silent).
    census_gen: AtomicU64,
    /// Woken alongside [`census_gen`] so the census task parks at zero cost while idle;
    /// a `Notify` holds at most one permit, so a burst coalesces to a single wake.
    census_dirty: Notify,
    /// Guards the single census task — spawned once, on the first `Info`+ enable.
    census_started: AtomicBool,
}

/// How long the census task lets process-state changes settle before emitting one
/// summary — a burst (e.g. a fan-out of workers) coalesces into a single line.
const CENSUS_DEBOUNCE: Duration = Duration::from_secs(2);

impl Inner {
    /// Whether the configured platform-log level is at least `level` — the single
    /// place the level rank is compared (the gate behind every lifecycle line).
    fn wants(&self, level: crate::LogLevel) -> bool {
        self.log_level.load(Ordering::Relaxed) >= level as u8
    }

    /// Record a labeled-process state change for the census: advance the generation
    /// (so the task knows something happened, even if counts net out unchanged) and wake
    /// it. The single place both spawn-naming and exit funnel through; call only when
    /// [`wants`](Self::wants)`(Info)` (the caller gates it, so off pays nothing).
    fn note_census(&self) {
        self.census_gen.fetch_add(1, Ordering::Relaxed);
        self.census_dirty.notify_one();
    }

    /// Enqueues `item` into `to`'s mailbox if it is still alive, keeping the
    /// mailbox-depth counter in step. The single place a mailbox grows — used by
    /// user sends, stream sends, and system deliveries alike. Returns whether it
    /// landed. (The exit cascade in [`propagate_exit`] enqueues inline because it
    /// already holds the entry lock.)
    fn enqueue(&self, to: Pid, item: Received, meta: Meta) -> bool {
        match self.table.get(&to.0) {
            Some(entry) => {
                // Opt-in overload protection: once a bounded mailbox is at
                // capacity, shed further *user* messages. System signals (exits,
                // monitor downs — delivered via `propagate_exit`/`deliver`) are
                // never shed, so back-pressure never breaks supervision.
                if let Some(cap) = self.mailbox_capacity {
                    if matches!(item, Received::Message(_)) && entry.depth_value() >= cap {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                }
                if entry
                    .mailbox
                    .send(Envelope {
                        received: item,
                        meta,
                    })
                    .is_ok()
                {
                    entry.note_enqueued();
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Delivers a system item to `to`'s mailbox if it is still alive. System items
    /// carry no metadata — only host-originated user sends attach a [`Meta`].
    fn deliver(&self, to: Pid, item: Received) {
        self.enqueue(to, item, None);
    }

    /// Removes `pid`, counts it finished, and fans its exit out to everyone who
    /// was watching: a [`Received::Down`] to each monitor and a propagated exit to
    /// each link. A staged cascade reason (see [`ProcessEntry::exit_reason`])
    /// overrides `reason`.
    fn deregister(&self, pid: Pid, reason: ExitReason) {
        let Some((_, entry)) = self.table.remove(&pid.0) else {
            return;
        };
        self.finished.fetch_add(1, Ordering::Relaxed);
        let reason = entry.exit_reason.unwrap_or(reason);

        // Platform log (opt-in): only labeled processes — i.e. components the host
        // named — so the stream is signal, not internal plumbing. `for_exit` is the one
        // place that maps a reason to its level (crash → Error, kill → Warn, clean → Info).
        if let Some(label) = &entry.label {
            if self.wants(crate::LogLevel::for_exit(reason)) {
                crate::lifecycle::log_exit(pid, label, reason);
            }
        }

        for name in &entry.names {
            self.registry.remove(name);
        }
        // Release process-group memberships the same way (and drop a group that empties),
        // so `whereis_tag`/`kill_tag` only ever see live members.
        let released_tags = !entry.tags.is_empty();
        for tag in &entry.tags {
            if let Some(mut members) = self.tags.get_mut(tag) {
                members.remove(&pid.0);
            }
            self.tags.remove_if(tag, |_, members| members.is_empty());
        }
        // Census activity on exit: a labeled process leaving changes a component count, and
        // a process releasing a tag changes that group's live count — the latter matters
        // even when the process is *unlabeled* (a per-connection serving process holds a
        // `room:`/feed tag but carries no label), so its disconnect still updates the
        // census. Mirrors the `register_tag`/`unregister_tag` triggers; debounced.
        if (entry.label.is_some() || released_tags) && self.wants(crate::LogLevel::Info) {
            self.note_census();
        }
        for monitor in entry.monitors {
            self.deliver(
                monitor.watcher,
                Received::Down {
                    reference: monitor.reference,
                    pid,
                    reason,
                },
            );
        }
        for peer in entry.links {
            self.propagate_exit(peer, pid, reason);
        }
    }

    /// Applies `from`'s exit to a linked `peer`: a trapping peer gets a
    /// [`Received::Exit`] message; an ordinary peer is taken down too on an
    /// abnormal exit (the cascade), carrying the same reason.
    fn propagate_exit(&self, peer: Pid, from: Pid, reason: ExitReason) {
        let Some(mut entry) = self.table.get_mut(&peer.0) else {
            return;
        };
        entry.links.retain(|&linked| linked != from);
        if entry.trap_exit {
            if entry
                .mailbox
                .send(Envelope::bare(Received::Exit { from, reason }))
                .is_ok()
            {
                entry.note_enqueued();
            }
        } else if reason.is_abnormal() {
            entry.exit_reason = Some(reason);
            entry.abort.abort();
        }
    }
}

/// Spawns and tracks lightweight processes. Cheap to clone — clones share the
/// same process table and counters.
#[derive(Clone, Default)]
pub struct Runtime {
    inner: Arc<Inner>,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Like [`new`](Runtime::new) but **tracks per-process mailbox depth**, so
    /// [`info`](Runtime::info) reports it. This costs a per-spawn counter
    /// allocation and a relaxed atomic per send/receive, so it's opt-in: enable it
    /// for an observer/REPL node; leave it off (the default) for peak throughput.
    pub fn with_mailbox_depth() -> Self {
        Self {
            inner: Arc::new(Inner {
                track_depth: true,
                ..Default::default()
            }),
        }
    }

    /// Like [`new`](Runtime::new) but **bounds each process mailbox** at `capacity`
    /// user messages. Once a mailbox holds that many, further *user* messages are
    /// **shed** — dropped and counted in
    /// [`dropped_messages`](Runtime::dropped_messages) — rather than growing memory
    /// without bound under a producer faster than its consumer (Erlang's
    /// `max_heap_size` / a bounded `GenStage`, in spirit).
    ///
    /// **System signals are never shed:** exit and monitor-down messages always
    /// land (they ride the same mailbox but bypass the capacity check), so overload
    /// back-pressure can never break links, monitors, or supervision. Enabling this
    /// also tracks mailbox depth — that count is how capacity is enforced.
    pub fn with_mailbox_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                track_depth: true,
                mailbox_capacity: Some(capacity),
                ..Default::default()
            }),
        }
    }

    /// Spawns a process running `body`, returning a handle to it. The body is a
    /// plain async closure today; in Phase 6 a Wasm instance becomes another
    /// kind of body behind the same API.
    pub fn spawn<F, Fut>(&self, body: F) -> ProcessHandle
    where
        F: FnOnce(Context) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.spawn_entry(Vec::new(), body).0
    }

    /// Like [`spawn`](Runtime::spawn), but the child is **linked** to `parent`
    /// before it runs — so the link is in place even if the child exits
    /// immediately, with no race (Erlang's `spawn_link`).
    pub fn spawn_link<F, Fut>(&self, parent: Pid, body: F) -> ProcessHandle
    where
        F: FnOnce(Context) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (handle, child) = self.spawn_entry(vec![parent], body);
        if let Some(mut entry) = self.inner.table.get_mut(&parent.0) {
            entry.links.push(child);
        }
        handle
    }

    fn spawn_entry<F, Fut>(&self, links: Vec<Pid>, body: F) -> (ProcessHandle, Pid)
    where
        F: FnOnce(Context) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let pid = Pid(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (mailbox, mailbox_rx) = unbounded_channel();
        let (abort, abort_registration) = AbortHandle::new_pair();
        // No allocation unless depth tracking is on (default off — see
        // `with_mailbox_depth`), keeping the spawn hot path allocation-lean.
        let depth = self
            .inner
            .track_depth
            .then(|| Arc::new(AtomicUsize::new(0)));

        // One write registers the whole process *before* it is spawned: a message
        // sent the instant after can't be lost, the reaper's remove always
        // balances this insert, and `kill`/`link` can already reach it.
        self.inner.table.insert(
            pid.0,
            ProcessEntry {
                abort: abort.clone(),
                mailbox,
                trap_exit: false,
                links,
                monitors: Vec::new(),
                names: Vec::new(),
                tags: Vec::new(),
                exit_reason: None,
                label: None,
                depth: depth.clone(),
            },
        );
        self.inner.spawned.fetch_add(1, Ordering::Relaxed);

        let body = body(Context {
            pid,
            mailbox: mailbox_rx,
            saved: VecDeque::new(),
            depth,
            current_meta: None,
            deferred: VecDeque::new(),
        });
        // The guard is moved *into* the task, so the process is deregistered on
        // every teardown path: completion, panic (drop runs during unwind), or a
        // kill (which makes `Abortable` resolve, ending the task).
        let guard = ProcessGuard {
            pid,
            inner: Arc::clone(&self.inner),
            reason: ExitReason::Killed,
        };
        let join = tokio::spawn(run(guard, Abortable::new(body, abort_registration)));
        (ProcessHandle { pid, abort, join }, pid)
    }

    /// Delivers `message` to `pid`'s mailbox. Returns `false` if there is no such
    /// live process — sending to a dead process is a silent no-op, like Erlang.
    pub fn send(&self, pid: Pid, message: Message) -> bool {
        self.inner.enqueue(pid, Received::Message(message), None)
    }

    /// Like [`send`](Self::send) but attaches opaque [`Meta`] that rides beside the
    /// message and surfaces to the recipient via [`Context::current_meta`] after it
    /// receives. The runtime never inspects `meta`; a host layer (`rusm-wasm`) boxes
    /// its own value here and downcasts it there — the seam that carries a request's
    /// claims context process-to-process without ever exposing it to guest code.
    pub fn send_with_meta(&self, pid: Pid, message: Message, meta: Meta) -> bool {
        self.inner.enqueue(pid, Received::Message(message), meta)
    }

    /// Delivers a byte `stream` to `pid` as a [`Received::Stream`]. Like
    /// [`send`](Runtime::send), returns `false` if there's no such live process.
    /// The recipient reads chunks at its own pace; back-pressure flows to the
    /// writer (the channel is bounded). The stream itself is the Wasm-free
    /// substrate the p3 component bridge maps `stream<u8>` onto.
    pub fn send_stream(&self, pid: Pid, stream: StreamHandle) -> bool {
        self.inner.enqueue(pid, Received::Stream(stream), None)
    }

    /// Number of currently-live processes.
    pub fn process_count(&self) -> usize {
        self.inner.table.len()
    }

    /// Total processes ever spawned.
    pub fn spawned(&self) -> u64 {
        self.inner.spawned.load(Ordering::Relaxed)
    }

    /// Total processes that have terminated (for any reason).
    pub fn finished(&self) -> u64 {
        self.inner.finished.load(Ordering::Relaxed)
    }

    /// Total user messages shed because a bounded mailbox was at capacity — always
    /// 0 unless built with [`with_mailbox_capacity`](Runtime::with_mailbox_capacity).
    /// A rising count is the signal that producers are outrunning a consumer.
    pub fn dropped_messages(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    pub fn is_alive(&self, pid: Pid) -> bool {
        self.inner.table.contains_key(&pid.0)
    }

    /// A snapshot of every live process's pid — Erlang's `Process.list/0`. Walks
    /// the sharded table without a global lock; a best-effort view (processes may
    /// spawn/exit during the walk).
    pub fn list(&self) -> Vec<Pid> {
        self.inner
            .table
            .iter()
            .map(|entry| Pid(*entry.key()))
            .collect()
    }

    /// A [`ProcessInfo`] snapshot for `pid`, or `None` if it isn't live —
    /// Erlang's `Process.info/1`. One table lookup; off the messaging hot path.
    pub fn info(&self, pid: Pid) -> Option<ProcessInfo> {
        self.inner.table.get(&pid.0).map(|entry| ProcessInfo {
            pid,
            links: entry.links.len(),
            monitors: entry.monitors.len(),
            names: entry.names.clone(),
            label: entry.label.clone(),
            mailbox_depth: entry
                .depth
                .as_ref()
                .map_or(0, |d| d.load(Ordering::Relaxed)),
            trap_exit: entry.trap_exit,
        })
    }

    /// Attaches a human-readable `label` to `pid` for observability (like
    /// Elixir's `Process.set_label/1`) — distinct from a registered name and
    /// need not be unique. Returns `false` if `pid` isn't live. One allocation,
    /// only when called; never touched on the send/receive path.
    pub fn set_label(&self, pid: Pid, label: impl Into<String>) -> bool {
        match self.inner.table.get_mut(&pid.0) {
            Some(mut entry) => {
                entry.label = Some(label.into());
                // A newly-named (or relabeled) process is census activity.
                if self.inner.wants(crate::LogLevel::Info) {
                    self.inner.note_census();
                }
                true
            }
            None => false,
        }
    }

    /// Set the **platform lifecycle log** level (spawn / exit / restart, coloured to
    /// stderr — see [`lifecycle`](crate::lifecycle)). `Off` by default; a node sets it
    /// explicitly (`rusm.toml [log] level = "debug"`). Set once at startup.
    pub fn set_log_level(&self, level: crate::LogLevel) {
        self.inner.log_level.store(level as u8, Ordering::Relaxed);
        // Bring up the single census task the first time logging reaches `Info`+ (the
        // level its per-component summary belongs to). It then parks on `census_dirty`
        // at no cost, and quiets itself if the level later drops (no more notifies).
        // Needs a Tokio runtime — skip cleanly if set outside one (e.g. a sync test).
        if level >= crate::LogLevel::Info
            && tokio::runtime::Handle::try_current().is_ok()
            && !self.inner.census_started.swap(true, Ordering::AcqRel)
        {
            self.spawn_census_loop();
        }
    }

    /// The per-component live-process **census**: every labeled process grouped by its
    /// label. Unlabeled processes (internal plumbing — responders, writers) are omitted,
    /// so the summary is about *components*. The single source of the count, shared by
    /// the census task and tests.
    pub(crate) fn census_counts(&self) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for entry in self.inner.table.iter() {
            if let Some(label) = &entry.label {
                *counts.entry(label.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// The **census of process groups**: each tag → how many live processes hold it (the
    /// tag-side companion to [`census_counts`](Self::census_counts)). Emptied groups are
    /// reaped on their last member's exit, so they never appear.
    pub(crate) fn tag_counts(&self) -> BTreeMap<String, u64> {
        self.inner
            .tags
            .iter()
            .filter(|members| !members.is_empty())
            .map(|members| (members.key().clone(), members.len() as u64))
            .collect()
    }

    /// One census step: emit a line iff a labeled-process change has happened **since the
    /// last emission** — tracked by the change generation, not by comparing counts, so a
    /// real spawn+exit (gen advanced, counts net-same) still logs, while a quiet stretch
    /// (gen unchanged) stays silent. `printed` is the caller's last-logged generation,
    /// updated in place. Returns whether a line was emitted. Factored out of the task so
    /// the decision is testable without the debounce timing.
    fn census_step(&self, printed: &mut u64) -> bool {
        let gen = self.inner.census_gen.load(Ordering::Relaxed);
        if gen == *printed {
            return false; // nothing happened since the last line — no duplicate
        }
        crate::lifecycle::log_census(&self.census_counts(), &self.tag_counts());
        *printed = gen;
        true
    }

    /// The debounced census task — one per runtime, started on the first `Info`+ enable.
    /// It parks until a change, lets the burst settle for [`CENSUS_DEBOUNCE`], then takes
    /// one [`census_step`](Self::census_step). Idle ⇒ parked (zero cost); a spawn storm ⇒
    /// one line; a spawn+exit ⇒ one line (activity, even if counts net out). `printed` is
    /// task-local — no shared state.
    fn spawn_census_loop(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut printed = 0u64;
            loop {
                runtime.inner.census_dirty.notified().await;
                tokio::time::sleep(CENSUS_DEBOUNCE).await;
                runtime.census_step(&mut printed);
            }
        });
    }

    /// Whether the configured level would log an event at `event` — a spawn site checks
    /// this before building a label/detail it would otherwise not need (off path free).
    pub fn wants_log(&self, event: crate::LogLevel) -> bool {
        self.inner.wants(event)
    }

    /// Emit a platform `spawn` log line (`detail` = the process's effective
    /// capabilities). The caller gates this with [`wants_log`](Self::wants_log)`(Debug)`
    /// and separately [`set_label`](Self::set_label)s the process (so the later `exit`
    /// line can name it even at levels below `Debug`).
    ///
    /// A **restart** needs no special event: it reads as the crashed instance's
    /// abnormal `exit` line followed by a fresh `spawn` line for the same component —
    /// with the crash reason and the new pid, which a bare "restart" couldn't carry.
    pub fn log_spawn(&self, pid: Pid, label: &str, detail: &str) {
        crate::lifecycle::log_spawn(pid, label, detail);
    }

    /// Stops `pid` at its next suspension point. Returns `false` if there is no
    /// such live process. Equivalent to `exit(pid, ExitReason::Killed)`. Logs the
    /// kill (yellow, at `Info`+) — the cause line ahead of the `exit` it triggers.
    pub fn kill(&self, pid: Pid) -> bool {
        if !self.terminate(pid) {
            return false;
        }
        if self.inner.wants(crate::LogLevel::Info) {
            crate::lifecycle::log_kill(pid);
        }
        true
    }

    /// Abort `pid` at its next suspension point **without logging** — the shared core
    /// of [`kill`](Self::kill) and [`kill_tag`](Self::kill_tag), which log at their own
    /// granularity (one `kill` line, or one `kill-tag` summary). `false` if not live.
    fn terminate(&self, pid: Pid) -> bool {
        match self.inner.table.get(&pid.0) {
            Some(entry) => {
                entry.abort.abort();
                true
            }
            None => false,
        }
    }

    /// Terminates `pid` with an explicit `reason` (Erlang's `exit/2`) — the
    /// reason links and monitors will observe. Lets a process "crash" without a
    /// Rust panic. Returns `false` if there is no such live process.
    pub fn exit(&self, pid: Pid, reason: ExitReason) -> bool {
        match self.inner.table.get_mut(&pid.0) {
            Some(mut entry) => {
                entry.exit_reason = Some(reason);
                entry.abort.abort();
                true
            }
            None => false,
        }
    }

    /// Sets whether `pid` traps exits. A trapping process receives a linked
    /// peer's exit as a [`Received::Exit`] message instead of dying with it — how
    /// a supervisor survives its children. No-op if `pid` is not alive.
    pub fn set_trap_exit(&self, pid: Pid, trap: bool) {
        if let Some(mut entry) = self.inner.table.get_mut(&pid.0) {
            entry.trap_exit = trap;
        }
    }

    /// Bidirectionally links two live processes: when either exits abnormally the
    /// other is taken down too (or, if it traps exits, gets a [`Received::Exit`]).
    /// A no-op if either is already dead or they are the same process.
    pub fn link(&self, a: Pid, b: Pid) {
        if a == b {
            return;
        }
        // Only link if both are live; record on each side. If one vanished
        // between the checks, undo so we never leave a half-link dangling.
        if self.add_link(a, b) {
            if self.add_link(b, a) {
                return;
            }
            self.remove_link(a, b);
        }
    }

    /// Removes the link between `a` and `b` in both directions.
    pub fn unlink(&self, a: Pid, b: Pid) {
        self.remove_link(a, b);
        self.remove_link(b, a);
    }

    fn add_link(&self, owner: Pid, peer: Pid) -> bool {
        match self.inner.table.get_mut(&owner.0) {
            Some(mut entry) => {
                if !entry.links.contains(&peer) {
                    entry.links.push(peer);
                }
                true
            }
            None => false,
        }
    }

    fn remove_link(&self, owner: Pid, peer: Pid) {
        if let Some(mut entry) = self.inner.table.get_mut(&owner.0) {
            entry.links.retain(|&linked| linked != peer);
        }
    }

    /// `watcher` starts monitoring `target`: when `target` exits, `watcher`
    /// receives a [`Received::Down`] carrying the returned reference and the exit
    /// reason. Monitoring is one-way and never propagates death. If `target` is
    /// already gone, the `Down` (reason [`ExitReason::NoProc`]) is delivered at
    /// once, like Erlang.
    pub fn monitor(&self, watcher: Pid, target: Pid) -> MonitorRef {
        let reference = MonitorRef(self.inner.next_ref.fetch_add(1, Ordering::Relaxed));
        match self.inner.table.get_mut(&target.0) {
            Some(mut entry) => entry.monitors.push(Monitor { watcher, reference }),
            None => self.inner.deliver(
                watcher,
                Received::Down {
                    reference,
                    pid: target,
                    reason: ExitReason::NoProc,
                },
            ),
        }
        reference
    }

    /// Registers `name` for `pid`, so it can be reached by name. Returns `false`
    /// if the name is already taken or `pid` is not alive. A pid may hold several
    /// names; a name maps to exactly one pid. Names are released automatically
    /// when the process exits (or via [`unregister`](Runtime::unregister)).
    pub fn register(&self, name: impl Into<String>, pid: Pid) -> bool {
        let name = name.into();
        // Hold the process entry first, then the registry slot — one consistent
        // lock order, so register can never deadlock against teardown.
        let Some(mut entry) = self.inner.table.get_mut(&pid.0) else {
            return false;
        };
        match self.inner.registry.entry(name.clone()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(pid.0);
                entry.names.push(name);
                true
            }
        }
    }

    /// Resolves a registered `name` to its (live) pid.
    pub fn whereis(&self, name: &str) -> Option<Pid> {
        self.inner.registry.get(name).map(|pid| Pid(*pid))
    }

    /// Returns the process registered under `name`, or spawns `body` and registers
    /// it under `name` — race-free. If concurrent callers race, exactly one wins the
    /// registration; the losers' just-spawned processes are killed and every caller
    /// gets the winner (Erlang's "whereis-or-start" / a registered singleton). The
    /// registration is the only synchronization point, so no lock is held across the
    /// spawn — `body` should be a long-lived process (a service/handler), not a
    /// one-shot. Use this to stand up a named singleton without a check-then-act race.
    pub fn whereis_or_spawn<F, Fut>(&self, name: impl Into<String>, body: F) -> Pid
    where
        F: FnOnce(Context) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let name = name.into();
        if let Some(pid) = self.whereis(&name) {
            return pid;
        }
        // Optimistically spawn a candidate, then claim the name atomically.
        let handle = self.spawn(body);
        if self.register(name.clone(), handle.pid()) {
            return handle.pid(); // we won — drop the handle; the registry keeps it live
        }
        // Lost the race: reap our candidate so it can't leak, and return the
        // incumbent the winner registered.
        handle.kill();
        self.whereis(&name).unwrap_or(handle.pid())
    }

    /// Releases `name`. Returns `false` if it wasn't registered.
    pub fn unregister(&self, name: &str) -> bool {
        match self.inner.registry.remove(name) {
            Some((_, pid)) => {
                if let Some(mut entry) = self.inner.table.get_mut(&pid) {
                    entry.names.retain(|held| held != name);
                }
                true
            }
            None => false,
        }
    }

    /// Adds `pid` to the process group `tag` (Erlang's `pg`): one pid may hold many tags,
    /// one tag many pids. Idempotent. Returns `false` only if `pid` is not alive. Tags are
    /// released automatically on exit (like names) or via
    /// [`unregister_tag`](Runtime::unregister_tag). This is the unprivileged half of the
    /// mechanism — a process tags *itself*; terminating a group ([`kill_tag`]) is where
    /// capability gating belongs, at the host ABI.
    ///
    /// [`kill_tag`]: Runtime::kill_tag
    pub fn register_tag(&self, tag: impl Into<String>, pid: Pid) -> bool {
        let tag = tag.into();
        // Same lock order as `register` — process entry first, then the tag map — so the
        // two can never deadlock against each other or against teardown.
        let Some(mut entry) = self.inner.table.get_mut(&pid.0) else {
            return false;
        };
        if self
            .inner
            .tags
            .entry(tag.clone())
            .or_default()
            .insert(pid.0)
        {
            entry.tags.push(tag);
            drop(entry); // release the table lock before waking the census task
                         // A new group membership changes the tag census.
            if self.inner.wants(crate::LogLevel::Info) {
                self.inner.note_census();
            }
        }
        true
    }

    /// Live members of process group `tag` (empty if unknown). The set holds only live
    /// pids — [`deregister`](Inner::deregister) removes a process from its tags on exit.
    pub fn whereis_tag(&self, tag: &str) -> Vec<Pid> {
        self.inner.tags.get(tag).map_or_else(Vec::new, |members| {
            members.iter().map(|&id| Pid(id)).collect()
        })
    }

    /// Removes `pid` from process group `tag`. Returns `false` if it wasn't a member.
    pub fn unregister_tag(&self, tag: &str, pid: Pid) -> bool {
        // Drop the tag-map lock before touching the table (mirrors `unregister`), so the
        // order here is never tag→table to clash with `register_tag`'s table→tag.
        let removed = self
            .inner
            .tags
            .get_mut(tag)
            .is_some_and(|mut members| members.remove(&pid.0));
        if removed {
            if let Some(mut entry) = self.inner.table.get_mut(&pid.0) {
                entry.tags.retain(|held| held != tag);
            }
            self.inner
                .tags
                .remove_if(tag, |_, members| members.is_empty());
            // Leaving a group changes the tag census.
            if self.inner.wants(crate::LogLevel::Info) {
                self.inner.note_census();
            }
        }
        removed
    }

    /// Terminates every live member of process group `tag`; returns how many were killed
    /// (`0` for an unknown/empty tag). Each member deregisters on exit, emptying the group
    /// — Erlang's "kill the whole `pg`". Members are snapshotted first, so one that dies
    /// concurrently is simply not counted, never double-killed.
    pub fn kill_tag(&self, tag: &str) -> usize {
        // `terminate` (not `kill`) per member → one `kill-tag` summary line, not N `kill`
        // lines (each member still logs its own `exit`).
        let killed = self
            .whereis_tag(tag)
            .into_iter()
            .filter(|&pid| self.terminate(pid))
            .count();
        if self.inner.wants(crate::LogLevel::Info) {
            crate::lifecycle::log_kill_tag(tag, killed);
        }
        killed
    }

    /// Sends to a registered `name`. Returns `false` if the name is unknown (or
    /// its process just died).
    pub fn send_named(&self, name: &str, message: Message) -> bool {
        match self.whereis(name) {
            Some(pid) => self.send(pid, message),
            None => false,
        }
    }

    /// Delivers `message` to `pid` after `delay`, returning a handle that can
    /// [`cancel`](TimerRef::cancel) it before it fires. Built on Tokio's timer
    /// wheel — many pending timers cost little, and cancellation is a free abort.
    pub fn send_after(&self, pid: Pid, delay: Duration, message: Message) -> TimerRef {
        let runtime = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            runtime.send(pid, message);
        });
        TimerRef {
            abort: task.abort_handle(),
        }
    }

    /// Like [`send_after`](Self::send_after) but the delivered message carries opaque
    /// [`Meta`] (captured now, attached when the timer fires) — so a delayed message a
    /// process schedules still arrives with the claims context current at schedule time.
    pub fn send_after_with_meta(
        &self,
        pid: Pid,
        delay: Duration,
        message: Message,
        meta: Meta,
    ) -> TimerRef {
        let runtime = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            runtime.send_with_meta(pid, message, meta);
        });
        TimerRef {
            abort: task.abort_handle(),
        }
    }

    /// Stops every live process (each still runs its normal teardown — links and
    /// monitors are notified, names released). Returns how many were signalled.
    /// Teardown is asynchronous; poll [`process_count`](Runtime::process_count)
    /// to wait for the drain.
    pub fn shutdown(&self) -> usize {
        // `abort()` only flips an atomic flag (it never touches the table), so it
        // is safe — and allocation-free — to signal each process during iteration.
        let mut stopped = 0;
        for entry in self.inner.table.iter() {
            entry.abort.abort();
            stopped += 1;
        }
        stopped
    }
}

/// A handle to a pending timer from [`send_after`](Runtime::send_after).
pub struct TimerRef {
    abort: tokio::task::AbortHandle,
}

impl TimerRef {
    /// Cancels the timer if it hasn't fired yet; a no-op once it has.
    pub fn cancel(&self) {
        self.abort.abort();
    }
}

/// Deregisters a process — and fans its exit out to links and monitors — on the
/// **Drop** path, so it runs however the body ends: completion, panic, or kill.
/// The guard lives inside the task (see [`Runtime::spawn_entry`]).
struct ProcessGuard {
    pid: Pid,
    inner: Arc<Inner>,
    reason: ExitReason,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // A panic unwinding through the task means the body crashed; otherwise
        // `run` has set the reason (Normal on completion, Killed on abort).
        let reason = if std::thread::panicking() {
            ExitReason::Crashed
        } else {
            self.reason
        };
        self.inner.deregister(self.pid, reason);
    }
}

async fn run<Fut>(mut guard: ProcessGuard, body: Abortable<Fut>)
where
    Fut: Future<Output = ()> + Send + 'static,
{
    // The guard lives in the task and deregisters the process on every exit path.
    // We only need to distinguish completion from a kill here; a panic is caught
    // by the guard via `std::thread::panicking()`. No select loop is needed — the
    // abort handle is the stop signal, and it drops the inner body future.
    guard.reason = match body.await {
        Ok(()) => ExitReason::Normal,
        Err(_aborted) => ExitReason::Killed,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_log_respects_the_configured_threshold() {
        let rt = Runtime::new();
        // Off by default: nothing is logged.
        assert!(!rt.wants_log(crate::LogLevel::Error));
        // At Warn: crashes (Error) and kills (Warn) log; clean exits / spawns don't.
        rt.set_log_level(crate::LogLevel::Warn);
        assert!(rt.wants_log(crate::LogLevel::Error));
        assert!(rt.wants_log(crate::LogLevel::Warn));
        assert!(!rt.wants_log(crate::LogLevel::Info));
        assert!(!rt.wants_log(crate::LogLevel::Debug));
        // At Debug: everything logs.
        rt.set_log_level(crate::LogLevel::Debug);
        assert!(rt.wants_log(crate::LogLevel::Debug));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn census_counts_live_processes_by_label() {
        let rt = Runtime::new();
        // Four parked processes; label two "alpha", one "beta", leave one unlabeled.
        let mut procs: Vec<_> = (0..4)
            .map(|_| {
                rt.spawn(|mut ctx| async move {
                    loop {
                        ctx.recv().await;
                    }
                })
            })
            .collect();
        assert!(rt.set_label(procs[0].pid(), "alpha"));
        assert!(rt.set_label(procs[1].pid(), "alpha"));
        assert!(rt.set_label(procs[2].pid(), "beta"));

        let counts = rt.census_counts();
        assert_eq!(counts.get("alpha"), Some(&2));
        assert_eq!(counts.get("beta"), Some(&1));
        assert_eq!(counts.len(), 2, "the unlabeled process is excluded");

        // When a labeled process exits, the census reflects it.
        let victim = procs.remove(1); // an "alpha"
        victim.kill();
        victim.join().await;
        assert_eq!(
            rt.census_counts().get("alpha"),
            Some(&1),
            "a drained process drops out of the census"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn census_step_emits_on_activity_not_on_count_equality() {
        let rt = Runtime::new();
        // The change generation only advances at Info+ (the level the census lives at).
        rt.set_log_level(crate::LogLevel::Info);
        fn park(mut ctx: Context) -> impl std::future::Future<Output = ()> {
            async move {
                loop {
                    ctx.recv().await;
                }
            }
        }
        let mut printed = 0u64;

        // A labeled spawn is activity → emits; nothing since → no duplicate.
        let a = rt.spawn(park);
        rt.set_label(a.pid(), "alpha");
        assert!(rt.census_step(&mut printed), "a labeled spawn emits");
        assert!(
            !rt.census_step(&mut printed),
            "nothing happened since → no duplicate line"
        );

        // A spawn+exit nets back to the same counts ({alpha:1}) — but processes genuinely
        // changed, so it must STILL emit (the case a count-comparison dedup wrongly hid).
        let b = rt.spawn(park);
        rt.set_label(b.pid(), "beta");
        b.kill();
        b.join().await;
        assert_eq!(
            rt.census_counts().get("alpha"),
            Some(&1),
            "counts netted back to the pre-spawn picture"
        );
        assert!(
            rt.census_step(&mut printed),
            "a net-zero spawn+exit is real activity → emits"
        );
        assert!(!rt.census_step(&mut printed), "and then stays quiet");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn census_tag_counts_reflect_group_membership() {
        let rt = Runtime::new();
        let procs: Vec<_> = (0..3)
            .map(|_| {
                rt.spawn(|mut ctx| async move {
                    loop {
                        ctx.recv().await;
                    }
                })
            })
            .collect();
        // Three processes in `plan:abc123`; one of them also in `plan:def456`.
        for p in &procs {
            assert!(rt.register_tag("plan:abc123", p.pid()));
        }
        assert!(rt.register_tag("plan:def456", procs[0].pid()));

        let tags = rt.tag_counts();
        assert_eq!(tags.get("plan:abc123"), Some(&3));
        assert_eq!(tags.get("plan:def456"), Some(&1));

        // kill_tag terminates the whole group; as each member drains it leaves both its
        // groups, so the emptied tags drop out of the census entirely.
        assert_eq!(rt.kill_tag("plan:abc123"), 3);
        for p in procs {
            p.join().await;
        }
        let tags = rt.tag_counts();
        assert_eq!(tags.get("plan:abc123"), None, "an emptied group drops out");
        assert_eq!(
            tags.get("plan:def456"),
            None,
            "its lone member died too, so it's gone as well"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn joining_or_leaving_a_group_is_census_activity() {
        let rt = Runtime::new();
        rt.set_log_level(crate::LogLevel::Info); // the census gen only advances at Info+
        let p = rt.spawn(|mut ctx| async move {
            loop {
                ctx.recv().await;
            }
        });
        let mut printed = 0u64;
        assert!(rt.register_tag("plan:x", p.pid()));
        assert!(
            rt.census_step(&mut printed),
            "joining a group is census activity → emits"
        );
        assert!(rt.unregister_tag("plan:x", p.pid()));
        assert!(
            rt.census_step(&mut printed),
            "leaving a group is census activity → emits"
        );
        assert!(!rt.census_step(&mut printed), "and then stays quiet");
    }

    #[tokio::test]
    async fn a_process_receives_a_message_sent_to_its_pid() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = rt.spawn(|mut ctx| async move {
            let msg = ctx.recv().await.message().unwrap();
            let _ = tx.send(msg);
        });
        assert!(rt.send(handle.pid(), b"hello".to_vec()));
        assert_eq!(rx.await.unwrap(), b"hello".to_vec());
        handle.join().await;
    }

    #[tokio::test]
    async fn messages_arrive_in_fifo_order() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = rt.spawn(|mut ctx| async move {
            let mut got = Vec::new();
            for _ in 0..3 {
                got.push(ctx.recv().await.message().unwrap());
            }
            let _ = tx.send(got);
        });
        for byte in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()] {
            assert!(rt.send(handle.pid(), byte));
        }
        assert_eq!(
            rx.await.unwrap(),
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
        );
        handle.join().await;
    }

    #[tokio::test]
    async fn recv_match_takes_a_match_and_leaves_the_rest_in_order() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = rt.spawn(|mut ctx| async move {
            // Want the "B" message; "A" arrives first and must be left queued.
            let matched = ctx
                .recv_match(|m| matches!(m, Received::Message(b) if b.first() == Some(&b'B')))
                .await
                .message()
                .unwrap();
            let then = ctx.recv().await.message().unwrap(); // the deferred "A"
            let last = ctx.recv().await.message().unwrap(); // then "C"
            let _ = tx.send((matched, then, last));
        });
        for m in [b"A".to_vec(), b"B".to_vec(), b"C".to_vec()] {
            assert!(rt.send(handle.pid(), m));
        }
        let (matched, then, last) = rx.await.unwrap();
        assert_eq!(matched, b"B".to_vec());
        assert_eq!(then, b"A".to_vec());
        assert_eq!(last, b"C".to_vec());
        handle.join().await;
    }

    #[tokio::test]
    async fn recv_match_finds_a_previously_deferred_message() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = rt.spawn(|mut ctx| async move {
            // Match "C" first, deferring A and B; then selectively pull B out of
            // the save queue, leaving A for an ordinary recv.
            let is = |byte: u8| move |m: &Received| matches!(m, Received::Message(b) if b.first() == Some(&byte));
            let c = ctx.recv_match(is(b'C')).await.message().unwrap();
            let b = ctx.recv_match(is(b'B')).await.message().unwrap();
            let a = ctx.recv().await.message().unwrap();
            let _ = tx.send((a, b, c));
        });
        for m in [b"A".to_vec(), b"B".to_vec(), b"C".to_vec()] {
            assert!(rt.send(handle.pid(), m));
        }
        let (a, b, c) = rx.await.unwrap();
        assert_eq!((a, b, c), (b"A".to_vec(), b"B".to_vec(), b"C".to_vec()));
        handle.join().await;
    }

    #[tokio::test]
    async fn send_to_unknown_pid_returns_false() {
        let rt = Runtime::new();
        assert!(!rt.send(Pid(424242), b"hi".to_vec()));
    }

    #[tokio::test]
    async fn send_to_a_finished_process_returns_false() {
        let rt = Runtime::new();
        let handle = rt.spawn(|_| async {});
        let pid = handle.pid();
        handle.join().await; // finished and reaped — mailbox is gone
        assert!(!rt.send(pid, b"too late".to_vec()));
    }

    #[tokio::test]
    async fn meta_rides_beside_a_message_and_a_plain_send_carries_none() {
        // The opaque-meta seam: `send_with_meta` surfaces to the recipient via
        // `current_meta` (downcast to the sender's own type), while a plain `send`
        // leaves it `None`. The runtime never interprets the value — it round-trips an
        // `Arc<dyn Any>`. This is the Wasm-free mechanism a host layer uses to carry a
        // request's claims context process-to-process, out of reach of the payload.
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            let first = ctx.recv().await.message().unwrap();
            let first_meta = ctx.current_meta().is_none();
            let second = ctx.recv().await.message().unwrap();
            let second_meta = ctx
                .current_meta()
                .and_then(|m| m.downcast_ref::<String>().cloned());
            let _ = tx.send((first, first_meta, second, second_meta));
        });
        rt.send(p.pid(), b"plain".to_vec());
        rt.send_with_meta(
            p.pid(),
            b"tagged".to_vec(),
            Some(Arc::new("acme".to_string())),
        );
        let (first, first_no_meta, second, second_meta) = rx.await.unwrap();
        assert_eq!(first, b"plain");
        assert!(first_no_meta, "a plain send carries no meta");
        assert_eq!(second, b"tagged");
        assert_eq!(second_meta.as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn stash_then_unstash_redelivers_with_each_item_s_own_meta() {
        // The host-side fix for the selective-receive leak: a guest SDK awaiting a reply
        // receives an unrelated message (tenant B), stashes it, then gets the reply (tenant
        // A). After `unstash`, the next `recv` must hand B back bound to **B's own** meta —
        // not A's. (Guest-side stashing replayed bytes without the meta, so B would have been
        // handled as A — a cross-tenant leak. Stashing host-side keeps the meta with the bytes.)
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            let read = |c: &Context| {
                c.current_meta()
                    .and_then(|m| m.downcast_ref::<String>().cloned())
            };
            // Awaiting the reply: first comes B's request — set it aside, keeping its meta.
            let stashed = ctx.recv().await.message().unwrap();
            let stashed_meta = read(&ctx);
            ctx.stash(stashed);
            // Then the reply (tenant A) — what we were waiting for.
            let reply = ctx.recv().await.message().unwrap();
            let reply_meta = read(&ctx);
            // Done waiting: restore the stashed mail and receive it as the next message.
            ctx.unstash();
            let replayed = ctx.recv().await.message().unwrap();
            let replayed_meta = read(&ctx);
            let _ = tx.send((stashed_meta, reply_meta, replayed, replayed_meta));
        });
        rt.send_with_meta(
            p.pid(),
            b"b-request".to_vec(),
            Some(Arc::new("tenant-b".to_string())),
        );
        rt.send_with_meta(
            p.pid(),
            b"reply".to_vec(),
            Some(Arc::new("tenant-a".to_string())),
        );
        let (stashed_meta, reply_meta, replayed, replayed_meta) = rx.await.unwrap();
        assert_eq!(stashed_meta.as_deref(), Some("tenant-b"));
        assert_eq!(reply_meta.as_deref(), Some("tenant-a"));
        assert_eq!(replayed, b"b-request");
        assert_eq!(
            replayed_meta.as_deref(),
            Some("tenant-b"),
            "the stashed message rebinds to its OWN tenant on replay, not the reply's"
        );
    }

    #[tokio::test]
    async fn selective_receive_preserves_each_saved_item_s_meta() {
        // A skipped (saved) item keeps *its own* meta for the later receive — meta is
        // never reordered or smeared across messages, even through the save queue.
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            // Take the second message first (skipping & saving the first), capturing meta.
            let picked = ctx
                .recv_match(|m| matches!(m, Received::Message(b) if b == b"second"))
                .await
                .message()
                .unwrap();
            let picked_meta = ctx
                .current_meta()
                .and_then(|m| m.downcast_ref::<String>().cloned());
            // Now drain the saved first message — its own meta must still be intact.
            let saved = ctx.recv().await.message().unwrap();
            let saved_meta = ctx
                .current_meta()
                .and_then(|m| m.downcast_ref::<String>().cloned());
            let _ = tx.send((picked, picked_meta, saved, saved_meta));
        });
        rt.send_with_meta(
            p.pid(),
            b"first".to_vec(),
            Some(Arc::new("tenant-a".to_string())),
        );
        rt.send_with_meta(
            p.pid(),
            b"second".to_vec(),
            Some(Arc::new("tenant-b".to_string())),
        );
        let (picked, picked_meta, saved, saved_meta) = rx.await.unwrap();
        assert_eq!(picked, b"second");
        assert_eq!(picked_meta.as_deref(), Some("tenant-b"));
        assert_eq!(saved, b"first");
        assert_eq!(saved_meta.as_deref(), Some("tenant-a"));
    }

    #[tokio::test]
    async fn tags_group_processes_and_kill_tag_terminates_only_that_group() {
        let rt = Runtime::new();
        let a = rt.spawn(|_| std::future::pending::<()>());
        let b = rt.spawn(|_| std::future::pending::<()>());
        let c = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register_tag("plan:1", a.pid()));
        assert!(rt.register_tag("plan:1", b.pid()));
        assert!(rt.register_tag("plan:2", c.pid())); // a separate group

        let mut g1 = rt.whereis_tag("plan:1");
        g1.sort_by_key(|p| p.0);
        let mut want = vec![a.pid(), b.pid()];
        want.sort_by_key(|p| p.0);
        assert_eq!(g1, want);
        assert_eq!(rt.whereis_tag("plan:2"), vec![c.pid()]);
        assert!(rt.whereis_tag("nope").is_empty());

        assert_eq!(rt.kill_tag("plan:1"), 2);
        a.join().await;
        b.join().await;
        assert!(rt.whereis_tag("plan:1").is_empty()); // members reaped to empty
        assert!(rt.is_alive(c.pid())); // the other group is untouched
        assert_eq!(rt.whereis_tag("plan:2"), vec![c.pid()]);
        c.kill();
        c.join().await;
    }

    #[tokio::test]
    async fn a_dead_process_leaves_its_tags_and_cannot_be_re_tagged() {
        let rt = Runtime::new();
        let a = rt.spawn(|_| std::future::pending::<()>());
        let pid = a.pid();
        assert!(rt.register_tag("g", pid));
        a.kill();
        a.join().await;
        assert!(rt.whereis_tag("g").is_empty()); // membership reaped on exit
        assert!(!rt.register_tag("g", pid)); // a dead pid can't be tagged
    }

    #[tokio::test]
    async fn a_process_holds_multiple_tags_and_can_leave_one() {
        let rt = Runtime::new();
        let a = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register_tag("x", a.pid()));
        assert!(rt.register_tag("y", a.pid()));
        assert!(rt.register_tag("x", a.pid())); // idempotent re-tag
        assert_eq!(rt.whereis_tag("x"), vec![a.pid()]);

        assert!(rt.unregister_tag("x", a.pid()));
        assert!(rt.whereis_tag("x").is_empty()); // emptied group is dropped
        assert_eq!(rt.whereis_tag("y"), vec![a.pid()]); // the other tag is intact
        assert!(!rt.unregister_tag("x", a.pid())); // already gone
        a.kill();
        a.join().await;
    }

    #[tokio::test]
    async fn an_unlabeled_tag_holder_exit_is_census_activity() {
        // A per-connection serving process is *unlabeled* but holds a process-group tag
        // (e.g. a chat `room:`). When it disconnects (exits), the tag's live count drops,
        // so the census must recount — even with no component label. Regression: the census
        // only re-fired on *labeled* exits, so chat/feed disconnects went unreported.
        let rt = Runtime::new();
        rt.set_log_level(crate::LogLevel::Info); // the census only tracks at Info+
        let a = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register_tag("room:general", a.pid()));
        let after_join = rt
            .inner
            .census_gen
            .load(std::sync::atomic::Ordering::Relaxed);

        a.kill(); // an unlabeled exit that releases the tag
        a.join().await;

        assert!(
            rt.inner
                .census_gen
                .load(std::sync::atomic::Ordering::Relaxed)
                > after_join,
            "an unlabeled tag-holder's exit advances the census generation (recount)"
        );
        assert!(
            rt.whereis_tag("room:general").is_empty(),
            "tag released on exit"
        );
    }

    #[tokio::test]
    async fn kill_tag_of_an_unknown_group_is_zero() {
        let rt = Runtime::new();
        assert_eq!(rt.kill_tag("ghost"), 0);
    }

    #[tokio::test]
    async fn killing_a_parked_receiver_stops_it_and_cleans_up() {
        // A process blocked in recv (no message will ever come) must still be
        // killable — abort wakes it at the suspension point and the guard reaps it.
        let rt = Runtime::new();
        let handle = rt.spawn(|mut ctx| async move {
            let _forever = ctx.recv().await;
        });
        let pid = handle.pid();
        assert!(rt.is_alive(pid));
        handle.kill();
        handle.join().await;
        assert!(!rt.is_alive(pid));
        assert_eq!(rt.finished(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_processes_play_ping_pong() {
        // A message carries its sender's pid (its first 8 bytes), so the ponger
        // knows whom to reply to — the byte-level analogue of Erlang's
        // `send(peer, {self(), :ping})`.
        let rt = Runtime::new();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        let ponger_rt = rt.clone();
        let ponger = rt.spawn(move |mut ctx| async move {
            let ball = ctx.recv().await.message().unwrap();
            let reply_to = Pid::from_raw(u64::from_le_bytes(ball[..8].try_into().unwrap()));
            ponger_rt.send(reply_to, b"pong".to_vec());
        });
        let ponger_pid = ponger.pid();

        let pinger_rt = rt.clone();
        let pinger = rt.spawn(move |mut ctx| async move {
            let mut ball = ctx.pid().raw().to_le_bytes().to_vec();
            ball.extend_from_slice(b"ping");
            pinger_rt.send(ponger_pid, ball);
            let _ = done_tx.send(ctx.recv().await.message().unwrap());
        });

        assert_eq!(done_rx.await.unwrap(), b"pong".to_vec());
        pinger.join().await;
        ponger.join().await;
    }

    #[tokio::test]
    async fn a_process_runs_to_completion_and_is_cleaned_up() {
        let rt = Runtime::new();
        let handle = rt.spawn(|_| async {});
        let pid = handle.pid();
        handle.join().await;
        assert_eq!(rt.spawned(), 1);
        assert_eq!(rt.finished(), 1);
        assert_eq!(rt.process_count(), 0);
        assert!(!rt.is_alive(pid));
    }

    #[tokio::test]
    async fn body_receives_its_own_pid() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = rt.spawn(move |ctx| async move {
            assert_eq!(
                format!("{ctx:?}"),
                format!("Context {{ pid: {:?} }}", ctx.pid())
            );
            let _ = tx.send(ctx.pid());
        });
        let pid = handle.pid();
        assert_eq!(rx.await.unwrap(), pid);
        handle.join().await;
    }

    #[tokio::test]
    async fn pids_are_unique_and_increasing() {
        let rt = Runtime::new();
        let a = rt.spawn(|_| async {});
        let b = rt.spawn(|_| async {});
        assert_ne!(a.pid(), b.pid());
        assert!(b.pid().raw() > a.pid().raw());
        a.join().await;
        b.join().await;
    }

    #[tokio::test]
    async fn kill_terminates_a_running_process() {
        let rt = Runtime::new();
        // A body that never completes on its own, so termination can only come
        // from the kill — `finished == 1` afterwards proves the kill worked.
        let handle = rt.spawn(|_| std::future::pending::<()>());
        let pid = handle.pid();
        assert!(rt.is_alive(pid));
        handle.kill();
        handle.join().await;
        assert!(!rt.is_alive(pid));
        assert_eq!(rt.process_count(), 0);
        assert_eq!(rt.finished(), 1);
    }

    #[tokio::test]
    async fn runtime_kill_signals_a_live_process() {
        let rt = Runtime::new();
        let handle = rt.spawn(|_| std::future::pending::<()>());
        let pid = handle.pid();
        assert!(rt.kill(pid));
        handle.join().await;
        assert!(!rt.is_alive(pid));
    }

    #[tokio::test]
    async fn kill_unknown_pid_returns_false() {
        let rt = Runtime::new();
        assert!(!rt.kill(Pid(999)));
    }

    #[tokio::test]
    async fn a_panicking_body_is_still_cleaned_up() {
        let rt = Runtime::new();
        let handle = rt.spawn(|_| async { panic!("boom") });
        handle.join().await; // join swallows the JoinError
        assert_eq!(rt.process_count(), 0);
        assert_eq!(rt.finished(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawns_many_processes_concurrently() {
        let rt = Runtime::new();
        let handles: Vec<_> = (0..1000).map(|_| rt.spawn(|_| async {})).collect();
        for handle in handles {
            handle.join().await;
        }
        assert_eq!(rt.spawned(), 1000);
        assert_eq!(rt.finished(), 1000);
        assert_eq!(rt.process_count(), 0);
    }

    // --- Phase 3: links, monitors, supervision -------------------------------

    /// A watcher process that forwards the first thing it receives to the test,
    /// then parks (staying alive so it can't race its own teardown). Returns its
    /// pid and the receiving end.
    fn watch(rt: &Runtime) -> (Pid, tokio::sync::oneshot::Receiver<Received>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let pid = rt
            .spawn(move |mut ctx| async move {
                let item = ctx.recv().await;
                let _ = tx.send(item);
                std::future::pending::<()>().await;
            })
            .pid();
        (pid, rx)
    }

    /// A process that parks until `go` fires, then ends the given way. The gate
    /// lets the test wire up links/monitors *before* the exit, with no sleeps.
    fn gated<F>(rt: &Runtime, ending: F) -> (Pid, tokio::sync::oneshot::Sender<()>)
    where
        F: FnOnce() + Send + 'static,
    {
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let pid = rt
            .spawn(move |_| async move {
                let _ = go_rx.await;
                ending();
            })
            .pid();
        (pid, go_tx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn monitor_reports_each_kind_of_exit() {
        let rt = Runtime::new();

        // Normal completion.
        let (w1, d1) = watch(&rt);
        let (t1, go1) = gated(&rt, || {});
        let r1 = rt.monitor(w1, t1);
        let _ = go1.send(());
        assert_eq!(
            d1.await.unwrap(),
            Received::Down {
                reference: r1,
                pid: t1,
                reason: ExitReason::Normal
            }
        );

        // Panic -> Crashed.
        let (w2, d2) = watch(&rt);
        let (t2, go2) = gated(&rt, || panic!("boom"));
        let r2 = rt.monitor(w2, t2);
        let _ = go2.send(());
        assert_eq!(
            d2.await.unwrap(),
            Received::Down {
                reference: r2,
                pid: t2,
                reason: ExitReason::Crashed
            }
        );

        // Kill -> Killed.
        let (w3, d3) = watch(&rt);
        let t3 = rt.spawn(|_| std::future::pending::<()>()).pid();
        let r3 = rt.monitor(w3, t3);
        assert!(rt.kill(t3));
        assert_eq!(
            d3.await.unwrap(),
            Received::Down {
                reference: r3,
                pid: t3,
                reason: ExitReason::Killed
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn monitoring_a_dead_process_reports_noproc_at_once() {
        let rt = Runtime::new();
        let dead = rt.spawn(|_| async {});
        let dead_pid = dead.pid();
        dead.join().await;

        let (watcher, down) = watch(&rt);
        let reference = rt.monitor(watcher, dead_pid);
        assert_eq!(
            down.await.unwrap(),
            Received::Down {
                reference,
                pid: dead_pid,
                reason: ExitReason::NoProc
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abnormal_exit_cascades_down_links_with_its_reason() {
        let rt = Runtime::new();
        let peer = rt.spawn(|_| std::future::pending::<()>()).pid();
        let (crasher, go) = gated(&rt, || panic!("boom"));
        rt.link(peer, crasher);

        // Watch the peer: it must go down too, carrying the *crash* reason, not
        // the bare Killed an abort would otherwise imply.
        let (watcher, down) = watch(&rt);
        let reference = rt.monitor(watcher, peer);

        let _ = go.send(());
        assert_eq!(
            down.await.unwrap(),
            Received::Down {
                reference,
                pid: peer,
                reason: ExitReason::Crashed
            }
        );
        assert!(!rt.is_alive(peer));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_normal_exit_does_not_cascade() {
        let rt = Runtime::new();
        let survivor = rt.spawn(|_| std::future::pending::<()>());
        let (quitter, go) = gated(&rt, || {});
        rt.link(survivor.pid(), quitter);

        let _ = go.send(());
        // Drain the quitter to completion; its teardown (and any propagation) has
        // run by the time the table no longer lists it.
        while rt.is_alive(quitter) {
            tokio::task::yield_now().await;
        }
        assert!(
            rt.is_alive(survivor.pid()),
            "a normal exit must not kill links"
        );
        survivor.kill();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_trapping_process_gets_an_exit_message_instead_of_dying() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let trapper = rt.spawn(move |mut ctx| async move {
            let item = ctx.recv().await;
            let _ = tx.send(item);
            std::future::pending::<()>().await; // stay alive to prove we trapped
        });
        rt.set_trap_exit(trapper.pid(), true);

        let (child, go) = gated(&rt, || panic!("boom"));
        rt.link(trapper.pid(), child);
        let _ = go.send(());

        assert_eq!(
            rx.await.unwrap(),
            Received::Exit {
                from: child,
                reason: ExitReason::Crashed
            }
        );
        assert!(
            rt.is_alive(trapper.pid()),
            "a trapping process must survive"
        );
        trapper.kill();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_link_links_the_child_to_its_parent() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let parent = rt.spawn(move |mut ctx| async move {
            let item = ctx.recv().await;
            let _ = tx.send(item);
            std::future::pending::<()>().await;
        });
        rt.set_trap_exit(parent.pid(), true);

        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let child = rt
            .spawn_link(parent.pid(), move |_| async move {
                let _ = go_rx.await;
                panic!("boom");
            })
            .pid();
        let _ = go_tx.send(());

        assert_eq!(
            rx.await.unwrap(),
            Received::Exit {
                from: child,
                reason: ExitReason::Crashed
            }
        );
        parent.kill();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unlinking_stops_propagation() {
        let rt = Runtime::new();
        let survivor = rt.spawn(|_| std::future::pending::<()>());
        let (crasher, go) = gated(&rt, || panic!("boom"));
        rt.link(survivor.pid(), crasher);
        rt.unlink(survivor.pid(), crasher);

        let _ = go.send(());
        while rt.is_alive(crasher) {
            tokio::task::yield_now().await;
        }
        assert!(
            rt.is_alive(survivor.pid()),
            "an unlinked peer must not be taken down"
        );
        survivor.kill();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn linking_a_dead_peer_leaves_no_half_link() {
        let rt = Runtime::new();
        let alive = rt.spawn(|_| std::future::pending::<()>());
        let dead = rt.spawn(|_| async {});
        let dead_pid = dead.pid();
        dead.join().await;

        // The dead side can't be recorded; the half-link on `alive` is undone.
        rt.link(alive.pid(), dead_pid);

        // Prove `alive`'s link set is intact: a fresh linked crasher still
        // cascades to it. (If the undo had corrupted the list this would hang.)
        let (crasher, go) = gated(&rt, || panic!("boom"));
        rt.link(alive.pid(), crasher);
        let _ = go.send(());
        while rt.is_alive(alive.pid()) {
            tokio::task::yield_now().await;
        }
        assert!(!rt.is_alive(crasher));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_terminates_with_the_chosen_reason() {
        let rt = Runtime::new();
        let (watcher, down) = watch(&rt);
        let target = rt.spawn(|_| std::future::pending::<()>()).pid();
        let reference = rt.monitor(watcher, target);

        // exit/2 with a custom reason — no panic, yet observed as Crashed.
        assert!(rt.exit(target, ExitReason::Crashed));
        assert_eq!(
            down.await.unwrap(),
            Received::Down {
                reference,
                pid: target,
                reason: ExitReason::Crashed
            }
        );
        assert!(!rt.exit(Pid::from_raw(987_654), ExitReason::Normal)); // unknown pid
    }

    #[tokio::test]
    async fn link_and_trap_on_missing_processes_are_no_ops() {
        let rt = Runtime::new();
        let p = rt.spawn(|_| std::future::pending::<()>());
        let dead = Pid::from_raw(999_999);
        rt.link(p.pid(), p.pid()); // self-link: ignored
        rt.link(dead, p.pid()); // dead first arg: nothing recorded
        rt.link(p.pid(), dead); // dead second arg: half-link undone
        rt.unlink(dead, p.pid()); // unlink with a dead owner: no-op
        rt.set_trap_exit(dead, true); // dead pid: no-op, no panic
        assert!(rt.is_alive(p.pid()));
        p.kill();
    }

    // --- Phase 4: registry, timers, shutdown ---------------------------------

    #[tokio::test]
    async fn register_whereis_send_named_then_auto_release_on_exit() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let worker = rt.spawn(move |mut ctx| async move {
            let job = ctx.recv().await.message().unwrap();
            let _ = tx.send(job);
        });
        assert!(rt.register("worker", worker.pid()));
        assert_eq!(rt.whereis("worker"), Some(worker.pid()));
        assert!(!rt.register("worker", worker.pid())); // already taken
        assert!(rt.send_named("worker", b"job".to_vec()));
        assert_eq!(rx.await.unwrap(), b"job".to_vec());

        worker.join().await; // exiting auto-releases the name
        assert_eq!(rt.whereis("worker"), None);
        assert!(!rt.send_named("worker", b"late".to_vec()));
    }

    #[tokio::test]
    async fn names_are_released_by_unregister_and_reusable_after_death() {
        let rt = Runtime::new();
        let a = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register("svc", a.pid()));
        assert!(rt.unregister("svc"));
        assert_eq!(rt.whereis("svc"), None);
        assert!(!rt.unregister("svc")); // already gone

        assert!(rt.register("svc", a.pid()));
        a.kill();
        a.join().await;
        assert_eq!(rt.whereis("svc"), None);
        let b = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register("svc", b.pid())); // a dead process's name is reusable
        b.kill();
    }

    #[tokio::test]
    async fn register_to_a_dead_pid_fails_and_a_pid_can_hold_several_names() {
        let rt = Runtime::new();
        let dead = rt.spawn(|_| async {});
        let dead_pid = dead.pid();
        dead.join().await;
        assert!(!rt.register("ghost", dead_pid));

        let p = rt.spawn(|_| std::future::pending::<()>());
        assert!(rt.register("one", p.pid()));
        assert!(rt.register("two", p.pid()));
        assert_eq!(rt.whereis("one"), Some(p.pid()));
        assert_eq!(rt.whereis("two"), Some(p.pid()));
        p.kill();
        p.join().await;
        assert_eq!(rt.whereis("one"), None); // all of a pid's names go on exit
        assert_eq!(rt.whereis("two"), None);
    }

    #[tokio::test]
    async fn whereis_or_spawn_returns_the_incumbent_without_spawning() {
        let rt = Runtime::new();
        let first = rt.whereis_or_spawn("svc", |_| std::future::pending::<()>());
        // A second call finds the registered process and does NOT start a new one.
        let before = rt.spawned();
        let again = rt.whereis_or_spawn("svc", |_| std::future::pending::<()>());
        assert_eq!(again, first, "must return the already-registered pid");
        assert_eq!(
            rt.spawned(),
            before,
            "must not spawn when the name is taken"
        );
        assert_eq!(rt.whereis("svc"), Some(first));
        rt.kill(first);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn whereis_or_spawn_is_race_free_and_kills_the_loser() {
        let rt = Runtime::new();
        // Many threads race to get-or-spawn the same name concurrently.
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let rt = rt.clone();
            tasks.push(tokio::spawn(async move {
                rt.whereis_or_spawn("singleton", |_| std::future::pending::<()>())
            }));
        }
        let mut pids = Vec::new();
        for t in tasks {
            pids.push(t.await.unwrap());
        }
        // Exactly one pid wins; everyone sees it.
        let winner = rt.whereis("singleton").expect("a winner is registered");
        assert!(
            pids.iter().all(|&p| p == winner),
            "all callers see the winner"
        );
        // Every loser it spawned was killed — only the winner remains live.
        loop {
            if rt.process_count() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(rt.whereis("singleton"), Some(winner));
        rt.kill(winner);
    }

    #[tokio::test(start_paused = true)]
    async fn send_after_delivers_when_the_timer_fires() {
        let rt = Runtime::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let target = rt.spawn(move |mut ctx| async move {
            let msg = ctx.recv().await.message().unwrap();
            let _ = tx.send(msg);
        });
        rt.send_after(target.pid(), Duration::from_secs(60), b"ding".to_vec());
        // Paused time auto-advances to the timer once everything else is idle.
        assert_eq!(rx.await.unwrap(), b"ding".to_vec());
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_timer_never_fires() {
        let rt = Runtime::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let target = rt.spawn(move |mut ctx| async move {
            loop {
                let _ = tx.send(ctx.recv().await);
            }
        });
        let timer = rt.send_after(target.pid(), Duration::from_secs(60), b"x".to_vec());
        timer.cancel();
        tokio::time::advance(Duration::from_secs(120)).await;
        tokio::task::yield_now().await; // let any (erroneous) delivery land before we check
        assert!(
            rx.try_recv().is_err(),
            "a cancelled timer must deliver nothing"
        );
        target.kill();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_stops_every_process() {
        let rt = Runtime::new();
        let procs: Vec<_> = (0..5)
            .map(|_| rt.spawn(|_| std::future::pending::<()>()))
            .collect();
        assert_eq!(rt.process_count(), 5);
        assert_eq!(rt.shutdown(), 5);
        for p in procs {
            p.join().await;
        }
        assert_eq!(rt.process_count(), 0);
        assert_eq!(rt.shutdown(), 0); // nothing left to stop
    }

    // --- Phase 7: introspection & labels -------------------------------------

    #[tokio::test]
    async fn list_reflects_live_processes() {
        use std::collections::HashSet;
        let rt = Runtime::new();
        assert!(rt.list().is_empty());
        let a = rt.spawn(|_| std::future::pending::<()>());
        let b = rt.spawn(|_| std::future::pending::<()>());
        let live: HashSet<u64> = rt.list().iter().map(|p| p.raw()).collect();
        assert_eq!(live, HashSet::from([a.pid().raw(), b.pid().raw()]));
        a.kill();
        a.join().await;
        assert_eq!(rt.list(), vec![b.pid()]);
        b.kill();
    }

    #[tokio::test]
    async fn info_reports_links_names_label_and_trap() {
        let rt = Runtime::new();
        let p = rt.spawn(|_| std::future::pending::<()>());
        let peer = rt.spawn(|_| std::future::pending::<()>());
        rt.link(p.pid(), peer.pid());
        assert!(rt.register("svc", p.pid()));
        rt.set_trap_exit(p.pid(), true);
        assert!(rt.set_label(p.pid(), "worker #1"));

        let info = rt.info(p.pid()).unwrap();
        assert_eq!(info.pid, p.pid());
        assert_eq!(info.links, 1);
        assert_eq!(info.monitors, 0);
        assert_eq!(info.names, vec!["svc".to_string()]);
        assert_eq!(info.label.as_deref(), Some("worker #1"));
        assert!(info.trap_exit);
        assert_eq!(info.mailbox_depth, 0);
        p.kill();
        peer.kill();
    }

    #[tokio::test]
    async fn info_and_set_label_on_a_dead_pid() {
        let rt = Runtime::new();
        let d = rt.spawn(|_| async {});
        let pid = d.pid();
        d.join().await;
        assert!(rt.info(pid).is_none());
        assert!(!rt.set_label(pid, "ghost"));
    }

    #[tokio::test]
    async fn mailbox_depth_tracks_unconsumed_messages() {
        let rt = Runtime::with_mailbox_depth();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            let _ = go_rx.await; // hold off consuming until the test has filled the box
            for _ in 0..3 {
                ctx.recv().await;
            }
            let _ = done_tx.send(());
            std::future::pending::<()>().await;
        });
        // `send` increments depth synchronously, so this is race-free even though
        // the process hasn't been polled past its gate yet.
        for m in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()] {
            assert!(rt.send(p.pid(), m));
        }
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 3);

        let _ = go_tx.send(());
        let _ = done_rx.await; // all three consumed by now
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 0);
        p.kill();
    }

    #[tokio::test]
    async fn a_bounded_mailbox_sheds_user_messages_past_capacity() {
        let rt = Runtime::with_mailbox_capacity(3);
        // A process that holds its mailbox open but never consumes it.
        let p = rt.spawn(|ctx| async move {
            let _hold = ctx; // keep the receiver alive (don't drain it)
            std::future::pending::<()>().await;
        });
        // `send` is synchronous, so this is race-free: the first three land, the
        // rest are shed once depth hits the capacity.
        for i in 0..10u8 {
            rt.send(p.pid(), vec![i]);
        }
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 3);
        assert_eq!(rt.dropped_messages(), 7);
        p.kill();
    }

    #[tokio::test]
    async fn a_full_mailbox_still_accepts_system_signals() {
        let rt = Runtime::with_mailbox_capacity(2);
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let (report_tx, report_rx) = tokio::sync::oneshot::channel();
        let watcher = rt.spawn(move |mut ctx| async move {
            let _ = go_rx.await; // stay parked until the test has filled the box
            let mut got = Vec::new();
            for _ in 0..3 {
                got.push(ctx.recv().await);
            }
            let _ = report_tx.send(got);
        });
        let wpid = watcher.pid();

        // A target the watcher monitors; killing it produces a Down for the watcher.
        let target = rt.spawn(|ctx| async move {
            let _hold = ctx;
            std::future::pending::<()>().await;
        });
        let tpid = target.pid();
        rt.monitor(wpid, tpid);

        // Fill the mailbox to capacity, then over it — the third is shed.
        assert!(rt.send(wpid, b"a".to_vec()));
        assert!(rt.send(wpid, b"b".to_vec()));
        assert!(!rt.send(wpid, b"c".to_vec()));
        assert_eq!(rt.dropped_messages(), 1);

        // Killing the target delivers a Down — a *system* signal, so it must land
        // even though the mailbox is at capacity. Depth rising to 3 proves it.
        rt.kill(tpid);
        for _ in 0..500 {
            if rt.info(wpid).map(|i| i.mailbox_depth) == Some(3) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert_eq!(
            rt.info(wpid).unwrap().mailbox_depth,
            3,
            "the Down landed despite the full mailbox"
        );

        // Released, the watcher sees both user messages and the Down (never "c").
        let _ = go_tx.send(());
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), report_rx)
            .await
            .expect("watcher never reported")
            .unwrap();
        let users = got
            .iter()
            .filter(|r| matches!(r, Received::Message(_)))
            .count();
        let downs = got
            .iter()
            .filter(|r| matches!(r, Received::Down { .. }))
            .count();
        assert_eq!(users, 2, "both queued user messages survive");
        assert_eq!(downs, 1, "the system Down was delivered, not shed");
    }

    #[tokio::test]
    async fn mailbox_depth_counts_messages_deferred_by_selective_receive() {
        let rt = Runtime::with_mailbox_depth();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            let _ = go_rx.await;
            // Consume only "B"; "A" and "C" stay deferred — still unconsumed.
            let _ = ctx
                .recv_match(|m| matches!(m, Received::Message(b) if b.first() == Some(&b'B')))
                .await;
            let _ = done_tx.send(());
            std::future::pending::<()>().await;
        });
        for m in [b"A".to_vec(), b"B".to_vec(), b"C".to_vec()] {
            assert!(rt.send(p.pid(), m));
        }
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 3);

        let _ = go_tx.send(());
        let _ = done_rx.await;
        // One consumed (B); A and C remain deferred but counted unconsumed.
        while rt.info(p.pid()).map_or(false, |i| i.mailbox_depth != 2) {
            tokio::task::yield_now().await;
        }
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 2);
        p.kill();
    }

    // --- Phase 7: stream-carrying messages -----------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stream_is_delivered_and_read_in_order_after_a_message() {
        use crate::stream::stream;
        let rt = Runtime::new();
        let (out_tx, out_rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            // A normal message, then a stream — FIFO across both kinds.
            let first = ctx.recv().await.message();
            let mut handle = ctx.recv().await.stream().expect("a stream");
            let mut chunks = Vec::new();
            while let Some(chunk) = handle.read().await {
                chunks.push(chunk);
            }
            let _ = out_tx.send((first, chunks));
        });

        assert!(rt.send(p.pid(), b"hello".to_vec()));
        let (writer, handle) = stream();
        assert!(rt.send_stream(p.pid(), handle));
        tokio::spawn(async move {
            for chunk in [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()] {
                writer.write(chunk).await.unwrap();
            }
            // Dropping the writer here closes the stream so the reader sees the end.
        });

        let (first, chunks) = out_rx.await.unwrap();
        assert_eq!(first, Some(b"hello".to_vec()));
        assert_eq!(chunks, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
        p.join().await;
    }

    #[tokio::test]
    async fn send_stream_to_a_dead_pid_returns_false() {
        use crate::stream::stream;
        let rt = Runtime::new();
        let (_writer, handle) = stream();
        assert!(!rt.send_stream(Pid::from_raw(123_456), handle));
    }

    #[tokio::test]
    async fn a_stream_counts_toward_mailbox_depth_until_consumed() {
        use crate::stream::stream;
        let rt = Runtime::with_mailbox_depth();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let p = rt.spawn(move |mut ctx| async move {
            let _ = go_rx.await;
            let _ = ctx.recv().await; // consume the stream message
            let _ = done_tx.send(());
            std::future::pending::<()>().await;
        });
        let (_writer, handle) = stream();
        assert!(rt.send_stream(p.pid(), handle));
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 1);
        let _ = go_tx.send(());
        let _ = done_rx.await;
        assert_eq!(rt.info(p.pid()).unwrap().mailbox_depth, 0);
        p.kill();
    }
}
