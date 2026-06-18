//! HTTP serving ergonomics for the **per-request** model. The host resolves the
//! `[routes]` table, spawns the matched handler component fresh **per request**, and
//! dispatches the matched action here; `#[rusm_rs::handlers]` turns a module of
//! `fn action(Request, Params) -> Response` into that component. (Server-Sent Events are
//! a per-connection handler now — see [`crate::sse`].)
//!
//! The [`Request`]/[`Response`] types (and their base64 body encoding) are the shared
//! [`rusm_wire`] definitions the host speaks — re-exported here so guest code never
//! drifts from the host.

pub use rusm_wire::{Request, Response};

// ── Per-request handlers: the unified serving model ──────────────────────────
//
// The host resolves the `[routes]` table, spawns this component fresh **per request**, and
// sends one `"fetch"` carrying the matched action, the captured path params, and the
// request. `#[rusm_rs::handlers]` dispatches it to the named handler function and replies;
// then the instance exits. All of this is *platform* code — an app author writes only
// `fn action(Request, Params) -> Response`, never the routing or the wire.

/// Path parameters captured from the route pattern (`/users/:id` → `params.get("id")`).
pub struct Params(Vec<(String, String)>);

impl Params {
    /// The value captured for `name`, or `None` if the route had no such parameter.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The wire the host sends a per-request handler: the matched `action`, captured path
/// `params`, the `request`, and the reply target (`from`/`ref`).
#[derive(serde::Deserialize)]
struct Incoming {
    action: String,
    #[serde(default)]
    params: Vec<(String, String)>,
    from: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<u64>,
    request: Request,
}

/// Send the head reply `{ref, ok: response}` to the responder pid.
fn reply_head(to: crate::Pid, reference: u64, response: &Response) {
    let reply = serde_json::json!({ "ref": reference, "ok": response });
    crate::send_bytes(to, &serde_json::to_vec(&reply).expect("reply serializes"));
}

/// Receive the one request the host dispatched, route it to a handler via `dispatch`, and
/// reply. Handles exactly one request — process-per-request — then returns so the instance
/// exits. Called by the `#[rusm_rs::handlers]`-generated entrypoint; `dispatch` returns
/// `None` for an unknown action (→ 404, though the host's router makes that unreachable).
pub fn serve_request(dispatch: impl FnOnce(&str, Request, Params) -> Option<Response>) {
    let Ok(inc) = serde_json::from_slice::<Incoming>(&crate::receive_bytes()) else {
        return;
    };
    let reply_to = inc
        .from
        .as_deref()
        .and_then(|f| f.parse().ok())
        .map(crate::Pid)
        .zip(inc.reference);
    let response = dispatch(&inc.action, inc.request, Params(inc.params));
    let Some((to, reference)) = reply_to else {
        return; // a cast (no reply target) can't be answered
    };
    let response = response.unwrap_or_else(|| Response::new(404, b"no such action".to_vec()));
    reply_head(to, reference, &response);
}
