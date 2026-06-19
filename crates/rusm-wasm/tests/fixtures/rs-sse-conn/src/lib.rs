//! A per-connection SSE handler that proves the `connection` actor op: on `open` it reads
//! its [`ConnectionInfo`](rusm_rs::ConnectionInfo) and reports the request method, path,
//! query, a captured route param, and a header to the registered `collector` — so a test
//! asserts the connection context reaches a per-connection handler end-to-end.
use rusm_rs::sse::{self, Handler, Stream};

struct Conn;

impl Handler for Conn {
    fn open(&mut self, stream: &Stream) {
        let info = stream.info();
        let report = format!(
            "{} {} q={} plan={} host={}",
            info.method(),
            info.path(),
            info.query(),
            info.param("plan").unwrap_or("-"),
            info.header("host").unwrap_or("?"),
        );
        if let Some(collector) = rusm_rs::whereis("collector") {
            rusm_rs::send_bytes(collector, report.as_bytes());
        }
    }

    // SSE is server→client only; no inbound events to handle for this context probe.
    fn message(&mut self, _stream: &Stream, _event: Vec<u8>) {}
}

#[rusm_rs::main]
fn run() {
    sse::serve(Conn);
}
