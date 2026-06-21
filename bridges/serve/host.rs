// Canonical source: bridges/serve/host.rs — the serve bridge's native host impl (the
// per-connection WS/SSE handler controls). Synced into rusm-wasm
// (crates/rusm-wasm/src/bridges/serve.rs) by `make sync-bridges`; edit this file, not the
// copy. The `bridge_host_in_sync` test fails the build on drift.

//! serve bridge — host side. Implements the `serve` WIT interface on [`WasiHost`]: a
//! per-connection WebSocket/SSE handler reads its [`ConnectionInfo`] and pushes frames/
//! events to the connection's writer over the bounded control channels (`ws_out`/`sse_out`)
//! the transport bridge wired at accept time. A normal process gets `none`/`false`. None of
//! these touch `rusm-otp`; they are per-instance store state + channel sends.

use crate::bindings::rusm::runtime::serve;
use crate::bridges::conn::{SseEvent, WsOut};
use crate::bridges::WasiHost;

/// The per-connection request context (method/path/query/params/headers/addr/subprotocol).
/// Re-exported so the transport bridges (`conn`/`http`/`ws`/`sse`) and the store can name it
/// without the full generated path.
pub(crate) use serve::ConnectionInfo;

/// Wire the serve interface into a component linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WasiHost>,
) -> wasmtime::Result<()> {
    serve::add_to_linker::<_, wasmtime::component::HasSelf<WasiHost>>(linker, |host| host)
}

impl serve::Host for WasiHost {
    /// This process's connection context, set by the serving bridge when it spawned a
    /// per-connection WebSocket/SSE handler; `None` for every other process. A plain
    /// read of per-instance store state — no `rusm-otp` call, no capability gate.
    async fn connection(&mut self) -> Option<ConnectionInfo> {
        self.connection.clone()
    }

    /// Send a text WebSocket frame to this connection's writer (binary frames use the
    /// plain `send` path). `false` if this process is not a WebSocket handler, or the
    /// bounded channel is closed (the client disconnected). The bound back-pressures a
    /// handler that outruns a slow client — it parks on `send` rather than buffering.
    async fn ws_send_text(&mut self, payload: Vec<u8>) -> bool {
        match &self.ws_out {
            Some(tx) => tx.send(WsOut::Text(payload)).await.is_ok(),
            None => false,
        }
    }

    /// Close this WebSocket connection with a status `code` + `reason` (a server-initiated
    /// close frame), then the connection tears down. No-op for a non-WebSocket process.
    async fn ws_close(&mut self, code: u16, reason: String) {
        if let Some(tx) = &self.ws_out {
            let _ = tx.send(WsOut::Close(code, reason)).await;
        }
    }

    /// Emit a rich SSE event (data + optional event/id/retry) to this connection's writer
    /// (the plain `data:` path is a `send` to the writer pid). `false` if this is not an SSE
    /// handler, or the bounded channel is closed (the client disconnected).
    async fn sse_send(
        &mut self,
        data: Vec<u8>,
        event: Option<String>,
        id: Option<String>,
        retry: Option<u32>,
    ) -> bool {
        match &self.sse_out {
            Some(tx) => tx
                .send(SseEvent {
                    data,
                    event,
                    id,
                    retry,
                })
                .await
                .is_ok(),
            None => false,
        }
    }
}
