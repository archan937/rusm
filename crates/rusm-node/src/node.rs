//! A running, **attachable** RUSM node: it observes a [`Runtime`] and streams
//! plain process introspection to attached clients over WebSocket, answering
//! their commands. Decision-making lives on [`Node`] (unit-testable without a
//! socket); the networking is in [`serve_on`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rusm_otp::Runtime;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{ClientCommand, NodeSnapshot, ProcessInfo, ServerMessage};
use crate::repl::{ReplHost, ReplSession};

/// An attachable node over a [`Runtime`]. Cheap to clone (the runtime is a
/// handle); wrap it in an [`Arc`] via [`Node::new`] so one ticker can fan out to
/// every connection.
pub struct Node {
    runtime: Runtime,
    name: String,
    /// Whether snapshots carry the per-process detail table (always-live counts
    /// are sent regardless). One relaxed atomic — toggled live by a client.
    detail: AtomicBool,
    started: Instant,
    tick_period: Duration,
    /// The JavaScript REPL engine, if one is wired in. `None` leaves the node
    /// observe-only (the bench node, or any node that doesn't opt into eval).
    repl: Option<Arc<dyn ReplHost>>,
}

impl Node {
    /// Builds an observe-only node over `runtime`, identified by `name`, sampling
    /// at `ticks_per_second` (clamped to ≥1). Detail is on by default; eval is off
    /// (no REPL host). See [`Node::with_repl`] to enable the live JS shell.
    pub fn new(runtime: Runtime, name: impl Into<String>, ticks_per_second: u32) -> Arc<Self> {
        Self::build(runtime, name, ticks_per_second, None)
    }

    /// Like [`Node::new`], plus a [`ReplHost`] so loopback clients can evaluate
    /// JavaScript against the live node (`rusm attach`'s shell).
    pub fn with_repl(
        runtime: Runtime,
        name: impl Into<String>,
        ticks_per_second: u32,
        repl: Arc<dyn ReplHost>,
    ) -> Arc<Self> {
        Self::build(runtime, name, ticks_per_second, Some(repl))
    }

    fn build(
        runtime: Runtime,
        name: impl Into<String>,
        ticks_per_second: u32,
        repl: Option<Arc<dyn ReplHost>>,
    ) -> Arc<Self> {
        let tick_period = Duration::from_millis(1_000 / u64::from(ticks_per_second.max(1)));
        Arc::new(Self {
            runtime,
            name: name.into(),
            detail: AtomicBool::new(true),
            started: Instant::now(),
            tick_period,
            repl,
        })
    }

    /// The REPL host, if eval is enabled on this node.
    pub fn repl(&self) -> Option<&Arc<dyn ReplHost>> {
        self.repl.as_ref()
    }

    /// The greeting sent on connect.
    pub fn hello(&self) -> ServerMessage {
        ServerMessage::Hello {
            node: self.name.clone(),
        }
    }

    pub fn tick_period(&self) -> Duration {
        self.tick_period
    }

    pub fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// A point-in-time snapshot of the observed runtime. The per-process detail
    /// table is included only when detail is enabled.
    pub fn snapshot(&self) -> NodeSnapshot {
        let processes = if self.detail.load(Ordering::Relaxed) {
            self.runtime
                .list()
                .into_iter()
                .filter_map(|pid| self.runtime.info(pid))
                .map(ProcessInfo::from)
                .collect()
        } else {
            Vec::new()
        };
        NodeSnapshot {
            uptime_ms: self.uptime_ms(),
            process_count: self.runtime.process_count(),
            processes,
        }
    }

    pub fn snapshot_message(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            snapshot: self.snapshot(),
        }
    }

    /// Applies a **synchronous** client command; `Err` carries a human-readable
    /// reason. [`ClientCommand::Eval`] is asynchronous and stateful, so it is
    /// routed through the REPL host in [`handle_connection`] rather than here —
    /// `apply` rejects it.
    pub fn apply(&self, command: ClientCommand) -> Result<(), String> {
        match command {
            ClientCommand::SetDetail { enabled } => {
                self.detail.store(enabled, Ordering::Relaxed);
                Ok(())
            }
            ClientCommand::Eval { .. } => Err("eval is handled by the REPL host, not apply".into()),
        }
    }
}

/// Binds `addr` and serves until error. See [`serve_on`].
pub async fn serve(addr: &str, node: Arc<Node>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    serve_on(listener, node).await
}

/// Serves on an already-bound listener (lets tests bind an ephemeral port). A
/// single ticker drives the node and broadcasts each snapshot; every connection
/// subscribes, so one tick fans out to all attached clients.
pub async fn serve_on(listener: TcpListener, node: Arc<Node>) -> std::io::Result<()> {
    let (tx, _) = broadcast::channel(64);
    tokio::spawn(ticker(node.clone(), tx.clone()));

    loop {
        let (stream, peer) = listener.accept().await?;
        // Eval is gated to loopback peers (the interim security model until an
        // authenticated channel exists); observe is allowed from anywhere.
        let eval_allowed = peer.ip().is_loopback();
        tokio::spawn(handle_connection(
            stream,
            node.clone(),
            tx.subscribe(),
            eval_allowed,
        ));
    }
}

async fn ticker(node: Arc<Node>, tx: broadcast::Sender<ServerMessage>) {
    let mut interval = tokio::time::interval(node.tick_period());
    loop {
        interval.tick().await;
        // Skip the snapshot work entirely when nobody is attached — an idle node
        // does no introspection and sends nothing.
        if tx.receiver_count() > 0 {
            let _ = tx.send(node.snapshot_message());
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    node: Arc<Node>,
    mut ticks: broadcast::Receiver<ServerMessage>,
    eval_allowed: bool,
) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut write, mut read) = ws.split();

    if send(&mut write, node.hello()).await.is_err() {
        return;
    }

    // The per-connection REPL session, opened lazily on the first accepted eval so
    // an observe-only client never spawns one. Dropped with the connection, which
    // tears down its process.
    let mut session: Option<Box<dyn ReplSession>> = None;

    loop {
        tokio::select! {
            tick = ticks.recv() => match tick {
                Ok(message) => {
                    if send(&mut write, message).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = read.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Some(reply) = handle_text(&node, eval_allowed, &mut session, text.as_str()).await {
                        if send(&mut write, reply).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}

/// Decide the reply (if any) for one inbound text frame — the unit-testable core of
/// the connection loop, kept out of the socket I/O. `None` means "send nothing" (a
/// successful synchronous command).
async fn handle_text(
    node: &Node,
    eval_allowed: bool,
    session: &mut Option<Box<dyn ReplSession>>,
    text: &str,
) -> Option<ServerMessage> {
    match ClientCommand::from_json(text) {
        Ok(ClientCommand::Eval { code }) => {
            Some(eval_line(node, eval_allowed, session, code).await)
        }
        // A synchronous command — today only `SetDetail`, which is infallible. The
        // `Err` mapping is a contract guard for any future fallible sync command.
        Ok(command) => node
            .apply(command)
            .err()
            .map(|message| ServerMessage::Error { message }),
        Err(error) => Some(ServerMessage::Error {
            message: error.to_string(),
        }),
    }
}

/// Resolves an `eval` request to the reply to send: enforces the gates (a REPL
/// host must be wired in, and the client must be loopback), opening the session
/// lazily on the first accepted line.
async fn eval_line(
    node: &Node,
    eval_allowed: bool,
    session: &mut Option<Box<dyn ReplSession>>,
    code: String,
) -> ServerMessage {
    let Some(host) = node.repl() else {
        return ServerMessage::Error {
            message: "eval is not enabled on this node".into(),
        };
    };
    if !eval_allowed {
        return ServerMessage::Error {
            message: "eval is local-only".into(),
        };
    }
    session
        .get_or_insert_with(|| host.open_session())
        .eval(code)
        .await
        .into_message()
}

async fn send<S>(write: &mut S, message: ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    write
        .send(Message::Text(message.to_json().into()))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::EvalOutcome;

    /// Spawns `n` parked processes on a fresh runtime and returns it.
    fn runtime_with(n: usize) -> Runtime {
        let rt = Runtime::new();
        for _ in 0..n {
            rt.spawn(|_| std::future::pending::<()>());
        }
        rt
    }

    /// A REPL host that echoes each line back as its value and counts how many
    /// sessions were opened — enough to drive the routing/gating logic.
    #[derive(Default)]
    struct EchoHost {
        sessions: std::sync::atomic::AtomicUsize,
    }

    struct EchoSession;

    impl ReplHost for EchoHost {
        fn open_session(&self) -> Box<dyn ReplSession> {
            self.sessions.fetch_add(1, Ordering::Relaxed);
            Box::new(EchoSession)
        }
    }

    impl ReplSession for EchoSession {
        fn eval(&mut self, code: String) -> crate::repl::EvalFuture<'_> {
            Box::pin(async move {
                EvalOutcome {
                    value: code,
                    ..Default::default()
                }
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn snapshot_reports_live_processes() {
        let node = Node::new(runtime_with(3), "n1", 20);
        let snap = node.snapshot();
        assert_eq!(snap.process_count, 3);
        // Detail is on by default: one entry per live process.
        assert_eq!(snap.processes.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_detail_off_keeps_counts_but_drops_the_table() {
        let node = Node::new(runtime_with(2), "n1", 20);
        node.apply(ClientCommand::SetDetail { enabled: false })
            .unwrap();
        let snap = node.snapshot();
        assert_eq!(snap.process_count, 2, "counts stay live");
        assert!(snap.processes.is_empty(), "detail table suppressed");
    }

    #[tokio::test]
    async fn hello_carries_the_node_name() {
        let node = Node::new(Runtime::new(), "my-app", 20);
        assert_eq!(node.hello().node(), Some("my-app"));
    }

    #[tokio::test]
    async fn apply_rejects_eval_which_is_routed_asynchronously() {
        let node = Node::new(Runtime::new(), "n", 20);
        let err = node
            .apply(ClientCommand::Eval { code: "1".into() })
            .unwrap_err();
        assert!(err.contains("REPL host"));
    }

    #[tokio::test]
    async fn eval_on_a_loopback_client_runs_against_the_repl_host() {
        let host = Arc::new(EchoHost::default());
        let node = Node::with_repl(Runtime::new(), "n", 20, host.clone());
        let mut session = None;
        let reply = eval_line(&node, true, &mut session, "1 + 1".into()).await;
        assert_eq!(
            reply,
            ServerMessage::EvalResult {
                value: "1 + 1".into(),
                output: vec![],
                error: None,
            }
        );
        // The session is opened once and reused for the connection's lifetime.
        let _ = eval_line(&node, true, &mut session, "2".into()).await;
        assert_eq!(host.sessions.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn eval_from_a_non_loopback_client_is_rejected_without_opening_a_session() {
        let host = Arc::new(EchoHost::default());
        let node = Node::with_repl(Runtime::new(), "n", 20, host.clone());
        let mut session = None;
        let reply = eval_line(&node, false, &mut session, "1".into()).await;
        assert_eq!(
            reply,
            ServerMessage::Error {
                message: "eval is local-only".into(),
            }
        );
        assert!(session.is_none());
        assert_eq!(host.sessions.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn eval_with_no_repl_host_reports_it_is_disabled() {
        let node = Node::new(Runtime::new(), "n", 20);
        let mut session = None;
        let reply = eval_line(&node, true, &mut session, "1".into()).await;
        assert_eq!(
            reply,
            ServerMessage::Error {
                message: "eval is not enabled on this node".into(),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_text_routes_eval_to_the_repl_host() {
        let node = Node::with_repl(Runtime::new(), "n", 20, Arc::new(EchoHost::default()));
        let mut session = None;
        let reply = handle_text(&node, true, &mut session, r#"{"type":"eval","code":"x"}"#).await;
        assert!(matches!(reply, Some(ServerMessage::EvalResult { .. })));
    }

    #[tokio::test]
    async fn handle_text_applies_a_sync_command_with_no_reply() {
        let node = Node::new(runtime_with(1), "n", 20);
        assert_eq!(node.snapshot().processes.len(), 1, "detail on by default");
        let mut session = None;
        let reply = handle_text(
            &node,
            true,
            &mut session,
            r#"{"type":"set_detail","enabled":false}"#,
        )
        .await;
        assert!(
            reply.is_none(),
            "a successful sync command sends nothing back"
        );
        assert!(
            node.snapshot().processes.is_empty(),
            "set_detail off took effect"
        );
    }

    #[tokio::test]
    async fn handle_text_rejects_a_malformed_frame() {
        let node = Node::new(Runtime::new(), "n", 20);
        let mut session = None;
        let reply = handle_text(&node, true, &mut session, "not json").await;
        assert!(matches!(reply, Some(ServerMessage::Error { .. })));
    }

    #[tokio::test]
    async fn handle_text_eval_without_a_host_is_rejected() {
        let node = Node::new(Runtime::new(), "n", 20);
        let mut session = None;
        let reply = handle_text(&node, true, &mut session, r#"{"type":"eval","code":"x"}"#).await;
        assert_eq!(
            reply,
            Some(ServerMessage::Error {
                message: "eval is not enabled on this node".into(),
            })
        );
    }

    #[tokio::test]
    async fn tick_period_derives_from_rate() {
        let node = Node::new(Runtime::new(), "n", 20);
        assert_eq!(node.tick_period(), Duration::from_millis(50));
    }

    #[tokio::test]
    async fn serve_returns_error_for_unbindable_address() {
        let node = Node::new(Runtime::new(), "n", 20);
        assert!(serve("definitely not an address", node).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_receives_hello_then_a_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let node = Node::new(runtime_with(1), "n1", 50);
        tokio::spawn(serve_on(listener, node));

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .unwrap();

        // First frame is the greeting.
        let Some(Ok(Message::Text(hello))) = ws.next().await else {
            panic!("expected hello text frame");
        };
        assert_eq!(ServerMessage::from_json(&hello).unwrap().node(), Some("n1"));

        // A snapshot arrives within a tick or two and reports the live process.
        let Some(Ok(Message::Text(tick))) = ws.next().await else {
            panic!("expected snapshot text frame");
        };
        let snap = ServerMessage::from_json(&tick).unwrap();
        assert_eq!(snap.snapshot().unwrap().process_count, 1);
    }
}
