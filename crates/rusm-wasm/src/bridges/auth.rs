//! **Serving authentication hooks** — the host-authoritative entry point that turns an
//! incoming request into a per-process [claims context](crate::ProcessContext).
//!
//! A hook runs *before* the handler is spawned: it inspects the request host-side
//! (verify a token in a header or query param, derive the tenant), and either returns
//! claims — which seed the request's context and ride it through the call graph to any
//! bridge — or rejects it, so the serving bridge replies `401` and never spawns a
//! handler. The hook is **host code** the operator registers per listener
//! (`[[serve]] authentication = "<name>"`); guest code never runs here and never sees the
//! claims. This is the seam that makes multi-tenant bridges safe: identity is established
//! by the operator, outside the guest's control.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rusm_otp::{Received, Runtime};
use serde::Serialize;

/// What an auth hook is shown about an incoming request (an HTTP request, or a WebSocket/
/// SSE upgrade). Assembled host-side by the serving bridge; never visible to guest code.
/// `Serialize` so a TS/Go auth hook receives it as JSON over the actor wire.
#[derive(Clone, Debug, Default, Serialize)]
pub struct AuthRequest {
    /// The HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// The request path, without the query string (`/orders/42`).
    pub path: String,
    /// The raw query string without the leading `?` (`token=…&page=2`), or `""`. A browser
    /// can't set `Authorization` on a WebSocket, so the token often arrives here instead.
    pub query: String,
    /// The request headers, in arrival order (`authorization`, `cookie`, …).
    pub headers: Vec<(String, String)>,
}

impl AuthRequest {
    /// The first header whose name equals `name` (ASCII case-insensitive), or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The value of query parameter `name` (first occurrence), or `None`. Handles plain
    /// `key=value` pairs separated by `&`; values are returned undecoded.
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == name).then_some(v)
        })
    }
}

/// An auth hook's verdict on a request.
pub enum AuthVerdict {
    /// Authenticated: these `key → value` claims seed the request's host-only context
    /// (e.g. `[("app_id", "acme")]`), reaching every bridge the handler's call graph hits.
    Allow(Vec<(String, String)>),
    /// Rejected: the serving bridge replies `401 Unauthorized` and never spawns a handler.
    Deny,
}

/// A serving auth hook: validates a request host-side and returns claims or a denial.
/// **Async**, so it may verify a token against a remote authority (a JWKS endpoint, an
/// introspection service) without blocking the accept loop. Registered per listener; the
/// `rusm-cli` app model compiles an operator's `auth/<name>/host.*` into one of these.
pub type AuthHook =
    Arc<dyn Fn(AuthRequest) -> Pin<Box<dyn Future<Output = AuthVerdict> + Send>> + Send + Sync>;

/// Build an [`AuthHook`] from an `async fn` (or async closure) — boxing its future so every
/// registered hook has the one uniform type. This is the constructor host code uses:
/// `wasm.register_auth_hook("jwt", rusm_wasm::auth_hook(authenticate))`. It exists so the
/// boxing+unsizing happens *here*, behind a named return type, rather than at each call site
/// (where `Arc::new(|req| Box::pin(f(req)))` does not reliably coerce to the trait object).
pub fn auth_hook<F, Fut>(f: F) -> AuthHook
where
    F: Fn(AuthRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = AuthVerdict> + Send + 'static,
{
    Arc::new(move |req| Box::pin(f(req)) as Pin<Box<dyn Future<Output = AuthVerdict> + Send>>)
}

/// Build an [`AuthHook`] that **delegates to a resident runner** — the TS/Go path. A TS/Go
/// `auth/<name>/host.*` compiles to a resident actor (registered as `runner`, e.g.
/// `auth:jwt`); this returns a hook that, per request, sends the [`AuthRequest`] as JSON over
/// the actor wire and awaits the runner's verdict. The single, tested place the TS/Go auth
/// delegation lives — the `rusm-cli` codegen just calls
/// `wasm.register_auth_hook("<name>", delegated_auth_hook(rt, "auth:<name>"))`.
///
/// **Fail-closed by construction:** if the runner isn't registered, dies, returns anything that
/// isn't an explicit `{"allow": {…}}`, **or doesn't answer within [`AUTH_HOOK_TIMEOUT`]**, the
/// verdict is [`AuthVerdict::Deny`] — a broken, missing, or hung hook never lets a request
/// through (so a stuck runner can't wedge every authenticated request). The runner speaks the
/// same `{fn, args, replyTo}` envelope as a custom-bridge runner (`fn = "authenticate"`,
/// `args = [request]`), so the TS/Go runner codegen is shared.
pub fn delegated_auth_hook(rt: Runtime, runner: String) -> AuthHook {
    delegate(rt, runner, AUTH_HOOK_TIMEOUT)
}

/// The core of [`delegated_auth_hook`], parameterised by the deny-on-no-answer `timeout` so
/// tests can drive the fail-closed-on-hang path without waiting the production ceiling.
fn delegate(rt: Runtime, runner: String, timeout: std::time::Duration) -> AuthHook {
    Arc::new(move |req: AuthRequest| {
        let rt = rt.clone();
        let runner = runner.clone();
        Box::pin(async move {
            // Runner not up → deny (fail-closed). It's a boot-supervised resident, so this is
            // only the brief window before it registers, or an operator misconfiguration.
            let Some(pid) = rt.whereis(&runner) else {
                return AuthVerdict::Deny;
            };
            static CALL_CTR: AtomicU64 = AtomicU64::new(0);
            // Colon-free call id (the reply is `"<call_id>:<json>"`; the verdict JSON contains
            // colons, so we split on the *first* one to recover it).
            let call_id = format!("rusm-auth-{}", CALL_CTR.fetch_add(1, Ordering::Relaxed));
            // A dedicated ephemeral responder receives the one reply and hands it to the oneshot
            // (no ref-matching needed — only the runner replies to this fresh pid).
            let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
            let responder = rt.spawn(move |mut ctx| async move {
                if let Received::Message(bytes) = ctx.recv().await {
                    let _ = tx.send(bytes);
                }
            });
            let args = serde_json::to_string(&[&req]).unwrap_or_else(|_| "[]".to_string());
            let envelope = format!(
                "{{\"fn\":\"authenticate\",\"args\":{args},\"replyTo\":{{\"pid\":\"{}\",\"callId\":\"{call_id}\"}}}}",
                responder.pid().raw(),
            );
            rt.send(pid, envelope.into_bytes());
            // Await the verdict, bounded: a runner that died/dropped the reply (`Err`) or never
            // answers (`timeout`) → deny. On timeout, reap the still-parked responder so a stuck
            // runner can't leak a process per request.
            let verdict = match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(reply)) => parse_verdict(&reply),
                _ => AuthVerdict::Deny,
            };
            responder.kill();
            verdict
        })
    })
}

/// The fail-closed ceiling for a delegated (TS/Go) auth hook: if its runner hasn't answered in
/// this long, the request is denied. Generous enough for a hook that verifies a token against a
/// remote authority (a JWKS fetch), short enough that a wedged runner can't pile requests up
/// forever. A Rust hook (no delegation) isn't bounded by this — it's a direct `async fn`.
const AUTH_HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Parse a delegated runner's reply (`"<call_id>:<json>"`) into a verdict. The JSON is the
/// runner's `authenticate` result: `{"allow": {claim: value, …}}` allows; **anything else**
/// (including a parse error, `{"deny": …}`, or an unexpected shape) denies — fail-closed.
fn parse_verdict(reply: &[u8]) -> AuthVerdict {
    let json = match reply.iter().position(|&b| b == b':') {
        Some(i) => &reply[i + 1..],
        None => reply,
    };
    match serde_json::from_slice::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(obj)) => match obj.get("allow") {
            Some(serde_json::Value::Object(claims)) => AuthVerdict::Allow(
                claims
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect(),
            ),
            _ => AuthVerdict::Deny,
        },
        _ => AuthVerdict::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive() {
        let req = AuthRequest {
            headers: vec![("Authorization".into(), "Bearer x".into())],
            ..Default::default()
        };
        assert_eq!(req.header("authorization"), Some("Bearer x"));
        assert_eq!(req.header("AUTHORIZATION"), Some("Bearer x"));
        assert_eq!(req.header("cookie"), None);
    }

    #[test]
    fn query_param_extracts_the_named_value() {
        let req = AuthRequest {
            query: "token=abc&page=2".into(),
            ..Default::default()
        };
        assert_eq!(req.query_param("token"), Some("abc"));
        assert_eq!(req.query_param("page"), Some("2"));
        assert_eq!(req.query_param("missing"), None);
    }

    // The exact shape `rusm build` generates: an `async fn authenticate` wrapped by
    // `auth_hook`. This compiles only if the helper boxes the future correctly — which is the
    // whole reason it exists (a bare `Arc::new(|req| Box::pin(authenticate(req)))` does not
    // reliably coerce to `AuthHook`). So this test is the compile-guard for the codegen.
    async fn authenticate(req: AuthRequest) -> AuthVerdict {
        match req.header("authorization") {
            Some("Bearer good") => AuthVerdict::Allow(vec![("app_id".into(), "acme".into())]),
            _ => AuthVerdict::Deny,
        }
    }

    #[tokio::test]
    async fn auth_hook_wraps_an_async_fn_into_a_callable_hook() {
        let hook: AuthHook = auth_hook(authenticate);
        let allowed = hook(AuthRequest {
            headers: vec![("authorization".into(), "Bearer good".into())],
            ..Default::default()
        })
        .await;
        assert!(
            matches!(allowed, AuthVerdict::Allow(claims) if claims == [("app_id".to_string(), "acme".to_string())])
        );
        let denied = hook(AuthRequest::default()).await;
        assert!(matches!(denied, AuthVerdict::Deny));
    }

    #[test]
    fn parse_verdict_allows_only_an_explicit_allow_object() {
        // Fail-closed parsing: only `{"allow": {…}}` allows; everything else denies.
        assert!(matches!(
            parse_verdict(br#"id:{"allow":{"app_id":"acme"}}"#),
            AuthVerdict::Allow(c) if c == [("app_id".to_string(), "acme".to_string())]
        ));
        assert!(matches!(
            parse_verdict(br#"id:{"deny":true}"#),
            AuthVerdict::Deny
        ));
        assert!(matches!(parse_verdict(br#"id:null"#), AuthVerdict::Deny));
        assert!(matches!(parse_verdict(b"id:not json"), AuthVerdict::Deny));
        assert!(matches!(parse_verdict(b"no-colon"), AuthVerdict::Deny));
    }

    /// A mock TS/Go-style runner: a resident actor that speaks the `{fn, args, replyTo}`
    /// envelope and replies `"<callId>:<verdict-json>"` — the exact protocol the generated
    /// runner uses. Lets us test `delegated_auth_hook` end to end without a real wasm runner.
    fn spawn_mock_runner(rt: &Runtime, name: &str, verdict_json: &'static str) {
        let replier = rt.clone();
        let handle = rt.spawn(move |mut ctx| async move {
            loop {
                let Some(bytes) = ctx.recv().await.message() else {
                    continue;
                };
                let Ok(env) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                let pid = env["replyTo"]["pid"]
                    .as_str()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                let call_id = env["replyTo"]["callId"].as_str().unwrap_or("");
                replier.send(
                    rusm_otp::Pid::from_raw(pid),
                    format!("{call_id}:{verdict_json}").into_bytes(),
                );
            }
        });
        rt.register(name, handle.pid());
        // Leak the handle so the resident keeps running for the test's duration.
        std::mem::forget(handle);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delegated_auth_hook_round_trips_a_verdict_and_fails_closed() {
        let rt = Runtime::new();

        // Runner not registered yet → deny (fail-closed).
        let hook = delegated_auth_hook(rt.clone(), "auth:jwt".to_string());
        assert!(matches!(
            hook(AuthRequest::default()).await,
            AuthVerdict::Deny
        ));

        // An allowing runner → the claims come back.
        spawn_mock_runner(&rt, "auth:jwt", r#"{"allow":{"app_id":"acme"}}"#);
        // Give the runner a tick to register.
        tokio::task::yield_now().await;
        let verdict = hook(AuthRequest {
            method: "GET".into(),
            path: "/me".into(),
            ..Default::default()
        })
        .await;
        assert!(
            matches!(verdict, AuthVerdict::Allow(c) if c == [("app_id".to_string(), "acme".to_string())]),
            "the runner's allow verdict round-trips with its claims"
        );

        // A denying runner → deny.
        spawn_mock_runner(&rt, "auth:deny", r#"{"deny":true}"#);
        tokio::task::yield_now().await;
        let deny_hook = delegated_auth_hook(rt.clone(), "auth:deny".to_string());
        assert!(matches!(
            deny_hook(AuthRequest::default()).await,
            AuthVerdict::Deny
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hung_runner_denies_after_the_timeout() {
        // Fail-closed on a wedged runner: it registers but never replies. The hook must deny
        // (after the timeout), not hang the request forever. Driven with a tiny timeout via the
        // private `delegate` so the test doesn't wait the production ceiling.
        let rt = Runtime::new();
        let handle = rt.spawn(|mut ctx| async move {
            loop {
                let _ = ctx.recv().await; // receive the request, never reply
            }
        });
        rt.register("auth:hang", handle.pid());
        std::mem::forget(handle);
        tokio::task::yield_now().await;

        let hook = delegate(
            rt.clone(),
            "auth:hang".to_string(),
            std::time::Duration::from_millis(50),
        );
        let started = std::time::Instant::now();
        let verdict = hook(AuthRequest::default()).await;
        assert!(matches!(verdict, AuthVerdict::Deny), "a hung runner denies");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "it denied promptly (the timeout), not the production ceiling"
        );
    }
}
