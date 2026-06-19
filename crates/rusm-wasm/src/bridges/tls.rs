//! Native **TLS** for serving (`https`/`wss`), opt-in per `[[serve]]` listener. A listener
//! with a `tls` cert/key terminates TLS on each accepted connection *before* hyper — so
//! HTTP, SSE, and WebSocket all run over TLS with no change to their per-connection logic
//! (they serve over any `AsyncRead + AsyncWrite`, plain or TLS). rustls + ring, the same
//! stack as the cluster transport. Host-only — never crosses into a guest.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;

pub use tokio_rustls::TlsAcceptor;

/// Build a [`TlsAcceptor`] from PEM-encoded cert chain + private key — the listener's
/// `tls = { cert, key }`. Uses the ring crypto provider explicitly, so it needs no
/// process-wide default installed.
pub fn tls_acceptor(cert_pem: &[u8], key_pem: &[u8]) -> anyhow::Result<TlsAcceptor> {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("reading TLS certificate PEM: {e}"))?;
    if certs.is_empty() {
        anyhow::bail!("the TLS certificate PEM contained no certificates");
    }
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut &key_pem[..])
        .map_err(|e| anyhow::anyhow!("reading TLS private key PEM: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("the TLS key PEM contained no private key"))?;

    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| anyhow::anyhow!("TLS configuration: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow::anyhow!("TLS configuration (cert/key mismatch?): {e}"))?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// An accepted serving connection — plain TCP, or TLS-terminated when the listener has a
/// `tls` config. Both implement `AsyncRead + AsyncWrite`, so the serving bridges drive
/// either uniformly through `hyper`/the WS codec.
pub(crate) enum MaybeTlsStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl MaybeTlsStream {
    /// Set `TCP_NODELAY` and, when the listener has a `tls` acceptor, complete the TLS
    /// handshake. Called in the per-connection task (never the accept loop) so a slow
    /// handshake can't stall accepting other connections.
    pub(crate) async fn accept(
        stream: TcpStream,
        tls: &Option<Arc<TlsAcceptor>>,
    ) -> io::Result<Self> {
        stream.set_nodelay(true).ok();
        match tls {
            Some(acceptor) => Ok(MaybeTlsStream::Tls(Box::new(
                acceptor.accept(stream).await?,
            ))),
            None => Ok(MaybeTlsStream::Plain(stream)),
        }
    }
}

// `TcpStream` and `tokio_rustls::server::TlsStream` are both `Unpin`, so the enum is too —
// delegate the poll methods to whichever variant is live.
impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}
