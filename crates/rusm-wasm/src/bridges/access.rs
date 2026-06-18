//! Platform **access logging** for served requests — the runtime's own access log for
//! the HTTP / WS / SSE serving paths.
//!
//! One line per served request: `<time> rusm <proto> <method> <path> → <status>`, the
//! status coloured by class. Built on [`rusm_logfmt::platform_line`], so it reads as one
//! stream with the lifecycle and guest logs, and gated by the node `[log] level` (shown
//! at `info`+) — when logging is off the serving hot path pays a single atomic load. The
//! serving bridges ([`super::routed`], [`super::http`], [`super::sse`], [`super::ws`]) all
//! emit through here, so an HTTP request, an SSE stream, and a WS upgrade read the same.

use rusm_logfmt as fmt;
use rusm_otp::{LogLevel, Runtime};

/// The severity colour for an HTTP status: 5xx red, 4xx yellow, everything else (2xx/3xx,
/// and the 1xx WS `101`) green — so the access log is scannable at a glance.
fn status_colour(status: u16) -> &'static str {
    match status {
        500.. => fmt::ERROR,
        400.. => fmt::WARN,
        _ => fmt::OK,
    }
}

/// The access line for one served request: `rusm <proto> <method> <path> → <status>`.
fn request_line(proto: &str, method: &str, path: &str, status: u16) -> String {
    fmt::platform_line(
        fmt::LEVEL,
        proto,
        &format!(
            "{method} {path} {} {}",
            fmt::paint(fmt::DIM, "→"),
            fmt::paint(status_colour(status), &status.to_string()),
        ),
    )
}

/// Emit the access line for one served request, gated by the node `[log] level` (shown at
/// `info`+). When logging is off this is a single atomic load and return — the default
/// serving path pays nothing. One atomic `eprintln!` per line (no interleave).
pub(crate) fn log_request(rt: &Runtime, proto: &str, method: &str, path: &str, status: u16) {
    if rt.wants_log(LogLevel::Info) {
        eprintln!("{}", request_line(proto, method, path, status));
    }
}

/// Whether a response is a Server-Sent-Events stream — `content-type: text/event-stream`.
/// Lets the access log tag an SSE stream `sse` rather than `http`, since both ride the
/// same HTTP serving path.
pub(crate) fn is_event_stream(headers: &hyper::HeaderMap) -> bool {
    headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("text/event-stream"))
}

/// SSE transport correctness, applied to every served response so all three guest
/// languages behave identically: an event-stream must never be cached, yet none of the
/// SDKs set it. Add `Cache-Control: no-cache` when this is an event-stream that lacks
/// one — idempotent, and a guest's explicit value is left untouched.
pub(crate) fn ensure_no_cache<B>(response: &mut hyper::Response<B>) {
    if is_event_stream(response.headers())
        && !response
            .headers()
            .contains_key(hyper::header::CACHE_CONTROL)
    {
        response.headers_mut().insert(
            hyper::header::CACHE_CONTROL,
            hyper::header::HeaderValue::from_static("no-cache"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_colour_is_by_class() {
        assert_eq!(status_colour(101), fmt::OK); // WS upgrade
        assert_eq!(status_colour(200), fmt::OK);
        assert_eq!(status_colour(301), fmt::OK);
        assert_eq!(status_colour(404), fmt::WARN);
        assert_eq!(status_colour(503), fmt::ERROR);
    }

    #[test]
    fn request_line_carries_proto_method_path_and_status() {
        let l = request_line("http", "GET", "/home", 200);
        assert!(l.contains("http"), "proto");
        assert!(l.contains("GET"), "method");
        assert!(l.contains("/home"), "path");
        assert!(l.ends_with("200"), "status is last");
    }

    #[test]
    fn is_event_stream_detects_the_sse_content_type() {
        let mut h = hyper::HeaderMap::new();
        assert!(!is_event_stream(&h));
        h.insert(
            hyper::header::CONTENT_TYPE,
            "text/event-stream".parse().unwrap(),
        );
        assert!(is_event_stream(&h));
        h.insert(
            hyper::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        assert!(!is_event_stream(&h));
    }

    fn response_with(content_type: Option<&str>) -> hyper::Response<()> {
        let mut r = hyper::Response::new(());
        if let Some(ct) = content_type {
            r.headers_mut()
                .insert(hyper::header::CONTENT_TYPE, ct.parse().unwrap());
        }
        r
    }

    #[test]
    fn ensure_no_cache_only_marks_uncached_event_streams() {
        // An event-stream without Cache-Control gains `no-cache`.
        let mut sse = response_with(Some("text/event-stream"));
        ensure_no_cache(&mut sse);
        assert_eq!(
            sse.headers().get(hyper::header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );

        // A guest's explicit Cache-Control is preserved (idempotent / non-clobbering).
        let mut explicit = response_with(Some("text/event-stream"));
        explicit.headers_mut().insert(
            hyper::header::CACHE_CONTROL,
            "no-store, max-age=0".parse().unwrap(),
        );
        ensure_no_cache(&mut explicit);
        assert_eq!(
            explicit
                .headers()
                .get(hyper::header::CACHE_CONTROL)
                .unwrap(),
            "no-store, max-age=0"
        );

        // A non-SSE response is untouched.
        let mut plain = response_with(Some("application/json"));
        ensure_no_cache(&mut plain);
        assert!(!plain.headers().contains_key(hyper::header::CACHE_CONTROL));
    }
}
