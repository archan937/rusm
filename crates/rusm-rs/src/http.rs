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

// ── Outbound HTTP: fetch ─────────────────────────────────────────────────────
//
// A guest makes an outbound request over raw **`wasi:http`** — capability-gated at the host on
// the **network** capability (a sandboxed guest is refused). Reuses the shared [`Request`] /
// [`Response`]. It's blocking and **parks the fiber** (`pollable.block()` / `blocking_read`),
// never busy-polls — the proven path, not `wstd` (whose reactor stalls under the async
// component model). The app writes `http::fetch(&Request::post(url, body).header(…))`, never
// the wasi:http plumbing.

/// Split a URL into its `(scheme, authority, path-with-query)` parts; `None` without a
/// `scheme://`. Authority is `host[:port]`; the path defaults to `/`. Pure — host-tested.
/// Compiled only where it's used: the wasm32 `fetch` below and the host unit tests (a plain
/// host, non-test build — e.g. `cargo publish`'s verify — has neither, so it isn't dead there).
#[cfg(any(target_arch = "wasm32", test))]
fn split_url(url: &str) -> Option<(&str, String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
        None => (rest.to_string(), "/".to_string()),
    };
    Some((scheme, authority, path))
}

/// Perform an outbound HTTP request, returning the [`Response`]. Needs the **network**
/// capability (otherwise the host refuses with an error). Blocking — suspends the fiber under
/// back-pressure / until the response arrives, freeing the Tokio worker meanwhile.
///
/// Coverage: the pure URL parsing is host-unit-tested ([`split_url`]); this body links
/// `wasi:http`/`wasi:io` imports that only resolve under a runtime, so it can't be host-tested
/// and is instead exercised end-to-end by the `rusm-wasm` integration test
/// (`rs_http_fetch_reaches_a_server_when_granted_and_is_denied_when_sandboxed`) — the real
/// dispatch path plus the capability gate, against a live server. A deliberate split, not a gap.
#[cfg(target_arch = "wasm32")]
pub fn fetch(req: &Request) -> Result<Response, String> {
    use wasip2::http::outgoing_handler;
    use wasip2::http::types::{Fields, Method, OutgoingBody, OutgoingRequest, Scheme};
    use wasip2::io::streams::StreamError;

    let (scheme, authority, path) =
        split_url(&req.url).ok_or_else(|| format!("bad URL {:?}", req.url))?;
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        other => Scheme::Other(other.to_string()),
    };
    let method = match req.method.as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        "OPTIONS" => Method::Options,
        other => Method::Other(other.to_string()),
    };

    let fields = Fields::new();
    for (k, v) in &req.headers {
        fields
            .append(&k.to_string(), v.as_bytes())
            .map_err(|e| format!("header {k}: {e:?}"))?;
    }
    let out = OutgoingRequest::new(fields);
    out.set_method(&method).map_err(|_| "bad method")?;
    out.set_scheme(Some(&scheme)).map_err(|_| "bad scheme")?;
    out.set_authority(Some(&authority))
        .map_err(|_| "bad authority")?;
    out.set_path_with_query(Some(&path))
        .map_err(|_| "bad path")?;

    // Canonical wasi:http order: take the body handle, **dispatch**, then write the body, then
    // finish — a body written before `handle` never reaches the server.
    let out_body = out.body().map_err(|_| "no request body")?;
    let future = outgoing_handler::handle(out, None).map_err(|e| format!("dispatch: {e:?}"))?;
    if !req.body.is_empty() {
        let stream = out_body.write().map_err(|_| "no body stream")?;
        for chunk in req.body.chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|e| format!("write body: {e:?}"))?;
        }
    }
    OutgoingBody::finish(out_body, None).map_err(|e| format!("finish body: {e:?}"))?;

    // Park the fiber until the response head is ready — no busy poll.
    future.subscribe().block();
    let response = match future.get() {
        Some(Ok(Ok(response))) => response,
        Some(Ok(Err(code))) => return Err(format!("transport: {code:?}")),
        _ => return Err("response unavailable".to_string()),
    };
    let status = response.status();
    let headers = response
        .headers()
        .entries()
        .into_iter()
        .map(|(k, v)| (k, String::from_utf8_lossy(&v).into_owned()))
        .collect();

    let incoming = response.consume().map_err(|_| "consume body")?;
    let stream = incoming.stream().map_err(|_| "body stream")?;
    let mut body = Vec::new();
    loop {
        match stream.blocking_read(64 * 1024) {
            Ok(chunk) if chunk.is_empty() => break, // some hosts signal EOF as an empty read
            Ok(chunk) => body.extend_from_slice(&chunk),
            Err(StreamError::Closed) => break,
            Err(StreamError::LastOperationFailed(e)) => {
                return Err(format!("read body: {}", e.to_debug_string()))
            }
        }
    }
    Ok(Response {
        status,
        headers,
        body,
        stream: false,
    })
}

#[cfg(test)]
mod tests {
    use super::split_url;

    #[test]
    fn split_url_parses_scheme_authority_and_path() {
        assert_eq!(
            split_url("https://api.example.com/v1/items?q=1"),
            Some(("https", "api.example.com".into(), "/v1/items?q=1".into()))
        );
        assert_eq!(
            split_url("http://host:8080"),
            Some(("http", "host:8080".into(), "/".into())) // path defaults to "/"
        );
        assert_eq!(split_url("no-scheme/path"), None);
    }
}
