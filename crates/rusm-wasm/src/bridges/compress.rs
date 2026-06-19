//! Response **compression** (Phase 11 serving-maturity), opt-in per `[[serve]]` listener.
//! Host-only: bytes are compressed at the transport edge and never cross into a guest. The
//! routed HTTP path and the SSE event stream use **gzip** (the SSE writer flushes per event
//! so nothing buffers); WebSocket uses **permessage-deflate** (see [`super::ws`]). Only
//! compressible content types over a small threshold are touched, and a reply that already
//! declares a `content-encoding` is left untouched.

use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY};
use hyper::http::HeaderValue;
use hyper::Response;

/// The boxed response body the routed gateway produces (buffered `Full` or streamed).
type ResBody = http_body_util::combinators::BoxBody<Bytes, std::convert::Infallible>;

/// Below this many bytes gzip's header + framing outweighs the saving, so a small body is
/// sent as-is.
pub(crate) const MIN_GZIP_SIZE: usize = 256;

/// Whether the client's `Accept-Encoding` offers gzip — case-insensitive, honouring a `*`
/// wildcard and an explicit `q=0` refusal (`gzip;q=0` / `*;q=0` → not accepted).
pub(crate) fn accepts_gzip(accept_encoding: Option<&str>) -> bool {
    let Some(value) = accept_encoding else {
        return false;
    };
    value.split(',').any(|token| {
        let mut parts = token.split(';').map(str::trim);
        let coding = parts.next().unwrap_or("").to_ascii_lowercase();
        if coding != "gzip" && coding != "*" {
            return false;
        }
        // Default quality is 1; the coding is refused only by an explicit `q=0`.
        parts.all(
            |param| match param.to_ascii_lowercase().strip_prefix("q=") {
                Some(q) => q.parse::<f32>().map(|v| v > 0.0).unwrap_or(true),
                None => true,
            },
        )
    })
}

/// Whether a `content-type` names a text-like (well-compressing) payload — `text/*`, JSON,
/// JavaScript, XML, SVG, and `+json`/`+xml` structured-syntax suffixes. An already-compressed
/// type (images, video, archives) or an unknown type is left alone.
pub(crate) fn is_compressible(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else {
        return false;
    };
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("text/")
        || ct == "application/json"
        || ct == "application/javascript"
        || ct == "application/xml"
        || ct == "application/xhtml+xml"
        || ct == "image/svg+xml"
        || ct.ends_with("+json")
        || ct.ends_with("+xml")
}

/// gzip `data` in one shot (for a buffered response body).
pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(
        Vec::with_capacity(data.len() / 2 + 32),
        Compression::default(),
    );
    encoder
        .write_all(data)
        .expect("gzip to a Vec is infallible");
    encoder
        .finish()
        .expect("gzip finish to a Vec is infallible")
}

/// gzip a buffered routed response in place when enabled, the client accepts gzip, the body
/// is a compressible type over the size threshold, and it isn't already encoded. Collecting
/// the body is safe here: the routed `#[handlers]` path always replies buffered.
pub(crate) async fn maybe_gzip(
    response: Response<ResBody>,
    accept_gzip: bool,
    enabled: bool,
) -> Response<ResBody> {
    if !enabled
        || !accept_gzip
        || response.headers().contains_key(CONTENT_ENCODING)
        || !is_compressible(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
        )
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let data = body
        .collect()
        .await
        .expect("a buffered body never errors")
        .to_bytes();
    if data.len() < MIN_GZIP_SIZE {
        // Too small to be worth it — re-wrap the original bytes, unchanged.
        return Response::from_parts(parts, Full::new(data).boxed());
    }
    let compressed = gzip(&data);
    parts
        .headers
        .insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    // The body length changed; let hyper recompute it from the new body.
    parts.headers.remove(CONTENT_LENGTH);
    parts
        .headers
        .append(VARY, HeaderValue::from_static("accept-encoding"));
    Response::from_parts(parts, Full::new(Bytes::from(compressed)).boxed())
}

/// A streaming gzip encoder for the SSE body: each event is written and **flushed** (gzip
/// `Z_SYNC_FLUSH`), so the client decodes it at once rather than waiting for the stream to
/// end. `encode` returns the compressed bytes produced for a chunk; `finish` the gzip footer.
pub(crate) struct GzipStream {
    encoder: GzEncoder<Vec<u8>>,
}

impl GzipStream {
    pub(crate) fn new() -> Self {
        Self {
            encoder: GzEncoder::new(Vec::new(), Compression::default()),
        }
    }

    /// Compress `chunk`, flush so it's emitted immediately, and drain the bytes produced.
    pub(crate) fn encode(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.encoder
            .write_all(chunk)
            .expect("gzip write to a Vec is infallible");
        self.encoder
            .flush()
            .expect("gzip flush to a Vec is infallible");
        std::mem::take(self.encoder.get_mut())
    }

    /// Finish the stream, returning the trailing gzip bytes (the footer).
    pub(crate) fn finish(self) -> Vec<u8> {
        self.encoder
            .finish()
            .expect("gzip finish to a Vec is infallible")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn gunzip(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        GzDecoder::new(data).read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn accepts_gzip_parses_the_header() {
        assert!(accepts_gzip(Some("gzip, deflate, br")));
        assert!(accepts_gzip(Some("deflate, gzip")));
        assert!(accepts_gzip(Some("GZIP"))); // case-insensitive
        assert!(accepts_gzip(Some("*")));
        assert!(accepts_gzip(Some("gzip;q=0.5")));
        assert!(!accepts_gzip(Some("gzip;q=0"))); // explicitly refused
        assert!(!accepts_gzip(Some("*;q=0")));
        assert!(!accepts_gzip(Some("deflate, br")));
        assert!(!accepts_gzip(None));
    }

    #[test]
    fn compressible_types_are_recognised() {
        assert!(is_compressible(Some("text/html; charset=utf-8")));
        assert!(is_compressible(Some("application/json")));
        assert!(is_compressible(Some("application/ld+json")));
        assert!(is_compressible(Some("image/svg+xml")));
        assert!(is_compressible(Some("text/event-stream")));
        assert!(!is_compressible(Some("image/png")));
        assert!(!is_compressible(Some("application/octet-stream")));
        assert!(!is_compressible(None));
    }

    #[test]
    fn gzip_round_trips() {
        let data = b"the quick brown fox".repeat(50);
        assert_eq!(gunzip(&gzip(&data)), data);
    }

    #[test]
    fn gzip_stream_flushes_each_chunk_and_round_trips() {
        // Each `encode` produces decodable bytes mid-stream (flush), and the whole stream
        // concatenated round-trips — the property the SSE writer relies on.
        let mut stream = GzipStream::new();
        let mut wire = Vec::new();
        wire.extend(stream.encode(b"data: one\n\n"));
        assert!(
            !wire.is_empty(),
            "an event flushes compressed bytes at once"
        );
        wire.extend(stream.encode(b"data: two\n\n"));
        wire.extend(stream.finish());
        assert_eq!(gunzip(&wire), b"data: one\n\ndata: two\n\n");
    }
}
