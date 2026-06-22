//! A guest that makes one outbound GET via `rusm_rs::http::fetch` and reports the result —
//! exercising the SDK's outbound HTTP (raw `wasi:http`, capability-gated) without the guest
//! touching the wasi:http plumbing. Message 1 is the URL; message 2 is the reply-to pid.
use rusm_rs::http::{self, Request};

#[rusm_rs::main]
fn run() {
    let url = String::from_utf8(rusm_rs::receive_bytes()).unwrap_or_default();
    let Some(reply_to) = String::from_utf8(rusm_rs::receive_bytes())
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .map(rusm_rs::Pid)
    else {
        return;
    };
    let msg = match http::fetch(&Request::get(url)) {
        Ok(resp) => format!("{}|{}", resp.status, String::from_utf8_lossy(&resp.body)),
        Err(e) => format!("ERR:{e}"),
    };
    rusm_rs::send_bytes(reply_to, msg.as_bytes());
}
