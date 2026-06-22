//! Wire types shared by the RUSM host (`rusm-wasm`) and the Rust guest (`rusm-rs`)
//! for resident HTTP/SSE serving — **one** definition, so the two binaries can't
//! drift. A request/response body crosses the JSON actor wire as **base64** (compact
//! ~1.33× and binary-safe). The TypeScript runner speaks the same shapes in its own
//! bridge (a separate language — the single place that can't share this source).

use serde::{Deserialize, Serialize};

/// (De)serialize a byte body as a base64 string on the JSON wire.
mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(d)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

/// An HTTP request delivered to a resident handler. The host serializes it into the
/// `"fetch"` envelope; the guest deserializes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    /// The HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// The request target — path and query (e.g. `/items?q=1`).
    pub url: String,
    /// Header name/value pairs, in arrival order.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// The raw request body (base64 on the wire).
    #[serde(default, with = "b64")]
    pub body: Vec<u8>,
}

impl Request {
    /// A `GET` request to `url` (build an outbound request for `rusm_rs::http::fetch`).
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A `POST` request to `url` with a raw body.
    pub fn post(url: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Adds a header, builder-style.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// The response a resident handler returns. The guest serializes it as the reply's
/// `ok`; the host deserializes it. `stream` marks an SSE response whose body then
/// rides a byte stream rather than `body`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Header name/value pairs.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// The response body (base64 on the wire).
    #[serde(default, with = "b64")]
    pub body: Vec<u8>,
    /// `true` for a streamed (SSE) response — the head carries no `body`; events
    /// follow on a byte stream.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub stream: bool,
}

impl Response {
    /// A `200 OK` `text/plain` response.
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
            body: body.into().into_bytes(),
            stream: false,
        }
    }

    /// A response with an explicit status and raw body (no default headers).
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            stream: false,
        }
    }

    /// Adds a header, builder-style.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_round_trips_as_base64() {
        let resp = Response::new(200, vec![0u8, 255, 1, 254]).header("x", "y");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"body\":\"")); // base64 string, not a number array
        assert!(!json.contains("stream")); // false is skipped
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(back.body, vec![0u8, 255, 1, 254]);
        assert_eq!(back.headers, vec![("x".into(), "y".into())]);
    }

    #[test]
    fn request_defaults_and_base64_body() {
        let req: Request = serde_json::from_str(r#"{"method":"GET","url":"/"}"#).unwrap();
        assert!(req.headers.is_empty() && req.body.is_empty());
        let with_body: Request =
            serde_json::from_str(r#"{"method":"POST","url":"/","body":"aGk="}"#).unwrap();
        assert_eq!(with_body.body, b"hi");
    }
}
