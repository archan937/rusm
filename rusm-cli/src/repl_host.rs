//! The REPL host: evaluates `rusm attach` JavaScript lines against the live node.
//!
//! This is the composition point. It implements the Wasm-free `rusm_node`
//! [`ReplHost`] contract over the [`WasmRuntime`] that owns the JS engine, so the
//! node layer never depends on Wasmtime. Each session is a spawned REPL-session
//! process (a `Trusted` sandboxed worker — the operator's local `iex --remsh`); a
//! line is driven over the actor wire and its reply bridged back to the async caller
//! through a transient sink process and a oneshot channel.

use std::sync::Arc;
use std::time::Duration;

use rusm_node::{EvalFuture, EvalOutcome, ReplHost, ReplSession};
use rusm_otp::{Pid, ProcessHandle, Received, Runtime};
use rusm_wasm::{CapabilityProfile, WasmRuntime};
use serde::Deserialize;

/// How long a single line may run before the session is presumed wedged: it is
/// reset and the line reported as timed out. Generous — a real REPL line returns in
/// well under a millisecond; this only bounds a line that genuinely blocks (e.g.
/// `await Process.receive()` with no message ever arriving).
const EVAL_TIMEOUT: Duration = Duration::from_secs(30);

/// How a session spawns its process. Injected so [`WasmSession`] is testable with a
/// plain native process and no `WasmRuntime`; in production it is `spawn_repl_session`.
type SpawnSession = Box<dyn Fn() -> ProcessHandle + Send>;

/// A [`ReplHost`] over the node's [`WasmRuntime`]. Holds the runtime alive (shared
/// via `Arc`) for as long as the node serves.
pub struct WasmReplHost {
    wasm: Arc<WasmRuntime>,
    rt: Runtime,
}

impl WasmReplHost {
    pub fn new(wasm: Arc<WasmRuntime>, rt: Runtime) -> Self {
        Self { wasm, rt }
    }
}

impl ReplHost for WasmReplHost {
    fn open_session(&self) -> Box<dyn ReplSession> {
        let wasm = Arc::clone(&self.wasm);
        Box::new(WasmSession {
            rt: self.rt.clone(),
            spawn: Box::new(move || {
                wasm.spawn_repl_session(CapabilityProfile::Trusted.capabilities())
            }),
            timeout: EVAL_TIMEOUT,
            proc: None,
        })
    }
}

/// One attach connection's session: a single persistent JS process, spawned lazily
/// so an idle connection costs nothing, and respawned transparently after a reset.
struct WasmSession {
    rt: Runtime,
    spawn: SpawnSession,
    timeout: Duration,
    proc: Option<ProcessHandle>,
}

impl WasmSession {
    /// The session process's pid, spawning it on first use.
    fn pid(&mut self) -> Pid {
        self.proc.get_or_insert_with(&self.spawn).pid()
    }

    /// Tear down a (possibly wedged) session so the next line starts fresh.
    fn reset(&mut self) {
        if let Some(proc) = self.proc.take() {
            proc.kill();
        }
    }
}

impl Drop for WasmSession {
    fn drop(&mut self) {
        self.reset();
    }
}

impl ReplSession for WasmSession {
    fn eval(&mut self, code: String) -> EvalFuture<'_> {
        Box::pin(async move {
            let session = self.pid();
            match request_eval(&self.rt, session, self.timeout, &code).await {
                Some(outcome) => outcome,
                None => {
                    // No reply in time: the session may be wedged. Reset it (the next
                    // line respawns) and report the timeout.
                    self.reset();
                    EvalOutcome::from_error(format!(
                        "eval timed out after {}s — session reset",
                        self.timeout.as_secs()
                    ))
                }
            }
        })
    }
}

/// Drive one line on `session`: send `{code, replyTo}` to it and await its reply via a
/// transient sink process, bounded by `timeout`. `None` means no reply arrived in time
/// (the session is presumed wedged); the sink is then killed so it can't linger.
async fn request_eval(
    rt: &Runtime,
    session: Pid,
    timeout: Duration,
    code: &str,
) -> Option<EvalOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sink = rt.spawn(move |mut ctx| async move {
        if let Received::Message(bytes) = ctx.recv().await {
            let _ = tx.send(bytes);
        }
    });

    let request = serde_json::json!({ "code": code, "replyTo": sink.pid().raw().to_string() });
    rt.send(session, request.to_string().into_bytes());

    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(bytes)) => Some(parse_outcome(&bytes)),
        _ => {
            sink.kill();
            None
        }
    }
}

/// The session's JSON reply (`{value, output, error}`) → an [`EvalOutcome`].
fn parse_outcome(bytes: &[u8]) -> EvalOutcome {
    #[derive(Deserialize)]
    struct Reply {
        value: String,
        output: Vec<String>,
        error: Option<String>,
    }
    match serde_json::from_slice::<Reply>(bytes) {
        Ok(reply) => EvalOutcome {
            value: reply.value,
            output: reply.output,
            error: reply.error,
        },
        Err(e) => EvalOutcome::from_error(format!("malformed eval reply: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_reply() {
        let outcome = parse_outcome(br#"{"value":"42","output":["hi"],"error":null}"#);
        assert_eq!(outcome.value, "42");
        assert_eq!(outcome.output, ["hi"]);
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn parses_an_error_reply() {
        let outcome = parse_outcome(br#"{"value":"","output":[],"error":"Error: boom"}"#);
        assert_eq!(outcome.error.as_deref(), Some("Error: boom"));
    }

    #[test]
    fn a_malformed_reply_becomes_an_error_outcome() {
        let outcome = parse_outcome(b"not json");
        assert!(outcome.error.unwrap().contains("malformed eval reply"));
    }

    /// A stand-in "session" process that replies to each `{code, replyTo}` with a
    /// canned outcome echoing the code — exercises the request/reply bridge without
    /// the real js-runner.
    fn echo_session(rt: &Runtime) -> SpawnSession {
        let rt = rt.clone();
        Box::new(move || {
            let reply_rt = rt.clone();
            rt.spawn(move |mut ctx| async move {
                while let Received::Message(bytes) = ctx.recv().await {
                    let req: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    let raw: u64 = req["replyTo"].as_str().unwrap().parse().unwrap();
                    let code = req["code"].as_str().unwrap();
                    let reply = serde_json::json!({ "value": code, "output": [], "error": null });
                    reply_rt.send(Pid::from_raw(raw), reply.to_string().into_bytes());
                }
            })
        })
    }

    /// A stand-in session that never replies — forces the eval timeout path.
    fn silent_session(rt: &Runtime) -> SpawnSession {
        let rt = rt.clone();
        Box::new(move || rt.spawn(|_ctx| std::future::pending::<()>()))
    }

    fn session(rt: &Runtime, spawn: SpawnSession, timeout: Duration) -> WasmSession {
        WasmSession {
            rt: rt.clone(),
            spawn,
            timeout,
            proc: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eval_drives_a_line_through_the_session_and_parses_the_reply() {
        let rt = Runtime::new();
        let mut s = session(&rt, echo_session(&rt), Duration::from_secs(5));
        let outcome = s.eval("1 + 1".into()).await;
        assert_eq!(outcome.value, "1 + 1"); // the echo session returns the code as the value
        assert_eq!(outcome.error, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_that_never_replies_times_out_and_resets() {
        let rt = Runtime::new();
        let mut s = session(&rt, silent_session(&rt), Duration::from_millis(80));
        let outcome = s.eval("hang()".into()).await;
        assert!(outcome.error.unwrap().contains("timed out"));
        // The wedged process was torn down, so the next line would respawn a fresh one.
        assert!(s.proc.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_a_session_tears_down_its_process() {
        let rt = Runtime::new();
        let mut s = session(&rt, echo_session(&rt), Duration::from_secs(5));
        let _ = s.eval("1".into()).await; // spawn the process
        let pid = s.proc.as_ref().unwrap().pid();
        assert!(rt.info(pid).is_some(), "the session process is alive");
        drop(s);
        // Drop → reset → kill; the process is gone shortly after.
        for _ in 0..200 {
            if rt.info(pid).is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the session process outlived its session");
    }
}
