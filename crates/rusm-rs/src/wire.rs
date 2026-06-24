//! The RPC wire layer — the same JSON protocol rusm-ts uses, so a Rust client and
//! a TS service (or vice versa) interoperate: requests `{op, args, from, ref}`
//! (`stream: true` for a streaming call) → replies `{ref, ok}` | `{ref, err}`,
//! with `{op: "__cb", cbref, args}` callback messages. These back the generated
//! `#[service]` dispatch loop and the `#[client]` typed client.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{me, receive_bytes, receive_bytes_timeout, send_bytes, unstash_front, Pid, Stream};

/// A decoded service request.
#[derive(serde::Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
    pub from: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<u64>,
    #[serde(default)]
    pub stream: bool,
}

impl Request {
    /// Deserialize the positional args as a tuple `T` (e.g. `(i64, i64)`).
    pub fn args<T: DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_value(self.args.clone()).map_err(|e| e.to_string())
    }

    /// The caller's pid, if the request expects a reply.
    pub fn caller(&self) -> Option<Pid> {
        self.from.as_ref().and_then(|f| f.parse().ok()).map(Pid)
    }
}

/// Block for the next service request. Non-request mail is **skipped** — a malformed message
/// or a monitor `__down`: a `#[service]` is request/reply only. A service that must react to a
/// monitored process's death should use a [`Supervisor`](crate::supervisor::Supervisor) or a
/// hand-rolled receive loop (matching `__down` via [`down_pid`](crate::down_pid)), not the
/// `#[service]` dispatch loop.
pub fn next_request() -> Request {
    loop {
        if let Ok(req) = serde_json::from_slice::<Request>(&receive_bytes()) {
            return req;
        }
    }
}

/// Reply to a call with a value (a no-op for a cast — no `ref`).
pub fn reply_ok<T: Serialize>(req: &Request, value: &T) {
    if let (Some(reference), Some(to)) = (req.reference, req.caller()) {
        let msg = serde_json::json!({ "ref": reference, "ok": value });
        send_bytes(to, &serde_json::to_vec(&msg).expect("reply serializes"));
    }
}

/// Reply to a call with an error message.
pub fn reply_err(req: &Request, message: &str) {
    if let (Some(reference), Some(to)) = (req.reference, req.caller()) {
        let msg = serde_json::json!({ "ref": reference, "err": message });
        send_bytes(to, &serde_json::to_vec(&msg).expect("reply serializes"));
    }
}

fn next_ref() -> u64 {
    thread_local! {
        static REF: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    REF.with(|r| {
        let n = r.get() + 1;
        r.set(n);
        n
    })
}

fn send_request<A: Serialize>(to: Pid, op: &str, args: &A, reference: Option<u64>, stream: bool) {
    let mut req = serde_json::json!({ "op": op, "args": args, "from": me().0.to_string() });
    if let Some(r) = reference {
        req["ref"] = serde_json::json!(r);
    }
    if stream {
        req["stream"] = serde_json::json!(true);
    }
    send_bytes(to, &serde_json::to_vec(&req).expect("request serializes"));
}

/// A blocking **call**: send the request, then wait for the matching reply, setting any
/// unrelated mail aside so the app's own `receive` still sees it (in order), and dispatching
/// any callback invocations. `args` serializes as a JSON array.
pub fn call<A: Serialize, R: DeserializeOwned>(to: Pid, op: &str, args: &A) -> Result<R, String> {
    call_json(
        to,
        op,
        serde_json::to_value(args).map_err(|e| e.to_string())?,
    )
}

/// Like [`call`] but with pre-built JSON `args` (an array) — used when some
/// arguments are callbacks (`{ "__cb": id }` markers) rather than plain values.
pub fn call_json<R: DeserializeOwned>(
    to: Pid,
    op: &str,
    args: serde_json::Value,
) -> Result<R, String> {
    let reference = next_ref();
    let req =
        serde_json::json!({ "op": op, "args": args, "from": me().0.to_string(), "ref": reference });
    send_bytes(to, &serde_json::to_vec(&req).expect("request serializes"));
    // Selective receive: hold non-matching mail in a LOCAL buffer while we wait, then restore
    // it to the inbox front. We must NOT re-stash mid-loop — `receive_bytes` drains the inbox
    // first, so a re-stashed message would be re-read forever while the real reply waits in the
    // mailbox behind it (a hang for any service that also makes calls).
    let mut saved: Vec<Vec<u8>> = Vec::new();
    let outcome = loop {
        let raw = receive_bytes();
        let v = match serde_json::from_slice::<serde_json::Value>(&raw) {
            Ok(v) => v,
            Err(_) => {
                saved.push(raw); // not even JSON — leave it for the app
                continue;
            }
        };
        match classify_reply(&v, reference) {
            Disposition::Callback => dispatch_callback(&v),
            Disposition::Reply(Err(err)) => break Err(err),
            Disposition::Reply(Ok(ok)) => {
                break serde_json::from_value(ok).map_err(|e| e.to_string())
            }
            Disposition::NotMine => saved.push(raw), // a request/other message — for the app
        }
    };
    unstash_front(saved);
    outcome
}

/// How `call_json` should treat an inbound message while awaiting reply `reference`.
enum Disposition {
    /// A `{op:"__cb"}` callback invocation — dispatch it and keep waiting.
    Callback,
    /// Our reply: the inner `err` string, or the `ok` value to deserialize.
    Reply(Result<serde_json::Value, String>),
    /// Not ours — set it aside for the process's own `receive`.
    NotMine,
}

/// Classify an inbound message against the ref we're awaiting. A reply is matched by ref
/// **and shape**: it must carry `ok` or `err`. Matching on ref alone is unsound — `ref`
/// numbers are a per-process counter, so a concurrent **request** from another process (which
/// also carries a `ref`) can collide with ours; without the shape check `call_json` would read
/// that request as its reply (no `ok`/`err` → `null` → a deserialize error). This is the
/// load-bearing rule for a service that also makes calls (a GenServer calling a GenServer).
fn classify_reply(v: &serde_json::Value, reference: u64) -> Disposition {
    if v.get("op").and_then(serde_json::Value::as_str) == Some("__cb") {
        return Disposition::Callback;
    }
    let is_reply = v.get("ok").is_some() || v.get("err").is_some();
    if is_reply && v.get("ref").and_then(serde_json::Value::as_u64) == Some(reference) {
        if let Some(err) = v.get("err").and_then(serde_json::Value::as_str) {
            return Disposition::Reply(Err(err.to_string()));
        }
        return Disposition::Reply(Ok(v.get("ok").cloned().unwrap_or(serde_json::Value::Null)));
    }
    Disposition::NotMine
}

/// A **cast**: fire-and-forget (no reply awaited).
pub fn cast<A: Serialize>(to: Pid, op: &str, args: &A) {
    send_request(to, op, args, None, false);
}

// --- callbacks: a closure stays in the caller; the service's invocations come
// back as `{op:"__cb", cbref, args}` messages routed to it during a call. ---

thread_local! {
    static CALLBACKS: std::cell::RefCell<
        std::collections::HashMap<u64, Box<dyn FnMut(serde_json::Value)>>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Register a caller-side callback closure; returns its id (sent to the service as
/// a `{ "__cb": id }` marker). The generated client registers/unregisters these.
pub fn register_callback<F: FnMut(serde_json::Value) + 'static>(f: F) -> u64 {
    let id = next_ref();
    CALLBACKS.with(|c| c.borrow_mut().insert(id, Box::new(f)));
    id
}

/// Drop a registered callback (after the call returns).
pub fn unregister_callback(id: u64) {
    CALLBACKS.with(|c| c.borrow_mut().remove(&id));
}

fn dispatch_callback(v: &serde_json::Value) {
    let Some(id) = v.get("cbref").and_then(serde_json::Value::as_u64) else {
        return;
    };
    let arg = v
        .get("args")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first().cloned())
        .unwrap_or(serde_json::Value::Null);
    // Take the closure out while invoking it, so a re-entrant call can't double-borrow.
    let cb = CALLBACKS.with(|c| c.borrow_mut().remove(&id));
    if let Some(mut f) = cb {
        f(arg);
        CALLBACKS.with(|c| c.borrow_mut().insert(id, f));
    }
}

/// Service side: reconstruct a [`Callback`](crate::Callback) from the `{ "__cb": id }`
/// marker at `index` in the request args, targeting the caller.
pub fn callback<A: Serialize>(req: &Request, index: usize) -> crate::Callback<A> {
    let cbref = req
        .args
        .get(index)
        .and_then(|m| m.get("__cb"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    crate::Callback::__new(req.caller().unwrap_or(Pid(0)), cbref)
}

/// Service side: deserialize a single positional argument at `index`.
pub fn arg<T: DeserializeOwned>(req: &Request, index: usize) -> Result<T, String> {
    let value = req
        .args
        .get(index)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Service side of a **streaming** call: open a stream back to the caller and
/// write each item (one JSON value per chunk), then close.
pub fn reply_stream<T, I>(req: &Request, items: I)
where
    T: Serialize,
    I: IntoIterator<Item = T>,
{
    let Some(to) = req.caller() else { return };
    let Some(stream) = Stream::open(to) else {
        return;
    };
    for item in items {
        if !stream.write(&serde_json::to_vec(&item).expect("chunk serializes")) {
            break; // reader gone
        }
    }
    stream.close();
}

#[cfg(test)]
mod tests {
    use super::{classify_reply, Disposition};
    use serde_json::json;

    fn reply(v: &serde_json::Value, reference: u64) -> Option<Result<serde_json::Value, String>> {
        match classify_reply(v, reference) {
            Disposition::Reply(r) => Some(r),
            _ => None,
        }
    }

    #[test]
    fn a_matching_reply_is_taken() {
        let ok = json!({ "ref": 1, "ok": { "Ok": null } });
        assert_eq!(reply(&ok, 1), Some(Ok(json!({ "Ok": null }))));
        let err = json!({ "ref": 1, "err": "boom" });
        assert_eq!(reply(&err, 1), Some(Err("boom".to_string())));
    }

    #[test]
    fn an_ok_reply_with_a_null_value_is_still_a_reply() {
        // A `() ` return serializes its `ok` as `null`; the key is present, so it matches.
        assert_eq!(
            reply(&json!({ "ref": 1, "ok": null }), 1),
            Some(Ok(json!(null)))
        );
    }

    #[test]
    fn a_reply_for_a_different_ref_is_not_ours() {
        assert!(reply(&json!({ "ref": 2, "ok": null }), 1).is_none());
    }

    #[test]
    fn an_inbound_request_with_a_colliding_ref_is_not_mistaken_for_the_reply() {
        // The regression: a service awaiting reply ref=1 receives another process's *request*
        // that also carries ref=1 (per-process counters collide). It has `op`/`from`, no
        // `ok`/`err` — it must be left for the app, never read as the reply.
        let request = json!({ "op": "subscribe_app", "args": ["app"], "from": "7", "ref": 1 });
        assert!(matches!(classify_reply(&request, 1), Disposition::NotMine));
    }

    #[test]
    fn a_callback_invocation_is_dispatched_not_matched() {
        let cb = json!({ "op": "__cb", "cbref": 3, "args": [null] });
        assert!(matches!(classify_reply(&cb, 1), Disposition::Callback));
    }
}

/// Like [`call`] but fails with `Err("timeout")` if the reply doesn't arrive within
/// `timeout_ms` milliseconds. `args` serializes as a JSON array.
pub fn call_timeout<A: Serialize, R: DeserializeOwned>(
    to: Pid,
    op: &str,
    args: &A,
    timeout_ms: u64,
) -> Result<R, String> {
    call_json_timeout(
        to,
        op,
        serde_json::to_value(args).map_err(|e| e.to_string())?,
        timeout_ms,
    )
}

/// Like [`call_json`] but with a deadline: fails with `Err("timeout")` if no matching
/// reply arrives within `timeout_ms` milliseconds. Non-matching mail is held in a local
/// buffer during the wait and restored to the inbox front on return.
pub fn call_json_timeout<R: DeserializeOwned>(
    to: Pid,
    op: &str,
    args: serde_json::Value,
    timeout_ms: u64,
) -> Result<R, String> {
    use std::time::Instant;
    let reference = next_ref();
    let req =
        serde_json::json!({ "op": op, "args": args, "from": me().0.to_string(), "ref": reference });
    send_bytes(to, &serde_json::to_vec(&req).expect("request serializes"));
    let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut saved: Vec<Vec<u8>> = Vec::new();
    let outcome = loop {
        let remaining_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        if remaining_ms == 0 {
            break Err("timeout".to_string());
        }
        match receive_bytes_timeout(remaining_ms as u64) {
            None => break Err("timeout".to_string()),
            Some(raw) => {
                let v = match serde_json::from_slice::<serde_json::Value>(&raw) {
                    Ok(v) => v,
                    Err(_) => {
                        saved.push(raw);
                        continue;
                    }
                };
                match classify_reply(&v, reference) {
                    Disposition::Callback => dispatch_callback(&v),
                    Disposition::Reply(Err(err)) => break Err(err),
                    Disposition::Reply(Ok(ok)) => {
                        break serde_json::from_value(ok).map_err(|e| e.to_string())
                    }
                    Disposition::NotMine => saved.push(raw),
                }
            }
        }
    };
    unstash_front(saved);
    outcome
}

/// Client side of a **streaming** call: send a stream request, accept the stream
/// the service opens, and yield each decoded chunk (blocking on each, EOF ends it).
pub fn call_stream<R: DeserializeOwned>(
    to: Pid,
    op: &str,
    args: &impl Serialize,
) -> impl Iterator<Item = R> {
    send_request(to, op, args, None, true);
    let stream = Stream::accept();
    std::iter::from_fn(move || {
        stream
            .read()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    })
}
