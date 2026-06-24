//! A WebSocket **frame transport** with **permessage-deflate** (RFC 7692).
//!
//! No async Rust WebSocket library exposes the per-message `RSV1` bit permessage-deflate
//! needs — both `tokio-tungstenite` and `fastwebsockets` hard-reject `RSV1` frames at their
//! protocol layer, with no extension hook. So the frame transport here is ours, but built on
//! tungstenite's battle-tested frame **primitives** (`FrameHeader::parse`/`format`, the
//! `OpCode` coding) — only the message-assembly + deflate layer is new. Only the **server**
//! role is implemented (RUSM serves WebSockets; it never dials out): inbound client frames
//! are masked, outbound server frames are not.
//!
//! Compression policy: we negotiate **no-context-takeover both directions** (RFC 7692
//! §7.1.1) — each message is (de)compressed standalone, so there is no sliding-window state
//! to carry between messages. That bounds memory and keeps the codec simple and robust, at a
//! small ratio cost; it's a complete, interoperable permessage-deflate, not a subset.

use std::io::{self, Cursor};

use bytes::{Buf, Bytes, BytesMut};
use flate2::write::{DeflateDecoder, DeflateEncoder};
use flate2::Compression;
use futures_util::{SinkExt, StreamExt};
use std::io::Write;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Control, Data, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::FrameHeader;
use tokio_util::codec::{Decoder, Encoder, Framed};

/// The permessage-deflate extension token a client offers in `Sec-WebSocket-Extensions`.
const PERMESSAGE_DEFLATE: &str = "permessage-deflate";

/// The empty-block marker a DEFLATE sync-flush appends; permessage-deflate strips it on the
/// wire and the receiver appends it back before inflating (RFC 7692 §7.2.1 / §7.2.2).
const SYNC_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// One raw WebSocket frame (server view): the bits the message layer needs.
#[derive(Debug, Clone)]
pub(crate) struct RawFrame {
    fin: bool,
    rsv1: bool,
    opcode: OpCode,
    payload: Bytes,
}

/// A fully assembled WebSocket message handed to the connection logic.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WsMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// A close, with its optional code + reason.
    Close(Option<(u16, String)>),
}

/// Whether the client's `Sec-WebSocket-Extensions` offers permessage-deflate, and the
/// `Sec-WebSocket-Extensions` value to echo in the `101` when it does. We always answer with
/// no-context-takeover both ways, which any conforming client accepts.
pub(crate) fn negotiate_permessage_deflate(extensions: Option<&str>) -> Option<String> {
    let offered = extensions?
        .split(',')
        .any(|ext| ext.split(';').next().map(str::trim) == Some(PERMESSAGE_DEFLATE));
    offered.then(|| {
        format!("{PERMESSAGE_DEFLATE}; client_no_context_takeover; server_no_context_takeover")
    })
}

/// Compress one message with a standalone (no-context-takeover) DEFLATE + sync flush, then
/// strip the trailing empty-block marker — the permessage-deflate payload transform.
pub(crate) fn deflate_message(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.flush()?; // a sync flush — emits all data + the `SYNC_TAIL` marker
    let mut out = std::mem::take(encoder.get_mut());
    if out.ends_with(&SYNC_TAIL) {
        out.truncate(out.len() - SYNC_TAIL.len());
    }
    // RFC 7692 §7.2.3.6: an empty compressed payload is sent as a single 0x00 byte.
    if out.is_empty() {
        out.push(0x00);
    }
    Ok(out)
}

/// Inflate one permessage-deflate message: append the stripped sync marker, then raw-inflate.
/// `max` caps the decompressed size (a deflate-bomb guard) when a message-size limit is set.
pub(crate) fn inflate_message(data: &[u8], max: Option<usize>) -> io::Result<Vec<u8>> {
    let mut decoder = DeflateDecoder::new(Vec::new());
    decoder.write_all(data)?;
    decoder.write_all(&SYNC_TAIL)?;
    decoder.flush()?;
    let out = std::mem::take(decoder.get_mut());
    if let Some(max) = max {
        if out.len() > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decompressed message exceeds the size limit",
            ));
        }
    }
    Ok(out)
}

/// In-place WebSocket masking/unmasking — XOR each byte with the 4-byte key (RFC 6455 §5.3).
fn apply_mask(payload: &mut [u8], mask: [u8; 4]) {
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
}

fn protocol_error(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// A `tokio_util` codec for raw WebSocket frames, on tungstenite's frame primitives.
struct WsFrameCodec {
    /// Per-frame inbound size cap (the listener's `max_message_size`); `None` = unbounded.
    max_size: Option<usize>,
}

impl Decoder for WsFrameCodec {
    type Item = RawFrame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<RawFrame>> {
        // Parse just the header (tungstenite leaves the cursor after it, or rewinds on a
        // short read); `position` tells us how many header bytes to consume.
        let (header, len, header_len) = {
            let mut cursor = Cursor::new(&src[..]);
            match FrameHeader::parse(&mut cursor)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
            {
                Some((header, len)) => (header, len, cursor.position() as usize),
                None => return Ok(None), // need more bytes for the header
            }
        };
        if header.rsv2 || header.rsv3 {
            return Err(protocol_error("unsupported reserved bits (rsv2/rsv3)"));
        }
        let len = len as usize;
        if let Some(max) = self.max_size {
            if len > max {
                return Err(protocol_error("frame exceeds the size limit"));
            }
        }
        let total = header_len + len;
        if src.len() < total {
            src.reserve(total - src.len()); // hint the framer to read the rest
            return Ok(None);
        }
        src.advance(header_len);
        let mut payload = src.split_to(len);
        if let Some(mask) = header.mask {
            apply_mask(&mut payload, mask);
        }
        Ok(Some(RawFrame {
            fin: header.is_final,
            rsv1: header.rsv1,
            opcode: header.opcode,
            payload: payload.freeze(),
        }))
    }
}

impl Encoder<RawFrame> for WsFrameCodec {
    type Error = io::Error;

    fn encode(&mut self, frame: RawFrame, dst: &mut BytesMut) -> io::Result<()> {
        // Server frames are never masked.
        let header = FrameHeader {
            is_final: frame.fin,
            rsv1: frame.rsv1,
            rsv2: false,
            rsv3: false,
            opcode: frame.opcode,
            mask: None,
        };
        let mut head = Vec::with_capacity(10);
        header
            .format(frame.payload.len() as u64, &mut head)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        dst.extend_from_slice(&head);
        dst.extend_from_slice(&frame.payload);
        Ok(())
    }
}

type FramedConn<S> = Framed<S, WsFrameCodec>;

/// A server-side WebSocket connection: a `Framed` frame transport plus the permessage-deflate
/// flag. Split into a [`WsSink`] (the per-connection writer process owns it) and a
/// [`WsStream`] (the inbound reader loop), mirroring the previous `WebSocketStream` split.
pub(crate) struct WsConn<S> {
    framed: FramedConn<S>,
    deflate: bool,
    max_size: Option<usize>,
}

impl<S> WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Wrap an upgraded stream. `deflate` enables permessage-deflate (already negotiated in
    /// the handshake); `max_size` caps an inbound frame/message.
    pub(crate) fn new(io: S, deflate: bool, max_size: Option<usize>) -> Self {
        WsConn {
            framed: Framed::new(io, WsFrameCodec { max_size }),
            deflate,
            max_size,
        }
    }

    pub(crate) fn split(self) -> (WsSink<S>, WsStream<S>) {
        let (sink, stream) = self.framed.split();
        (
            WsSink {
                sink,
                deflate: self.deflate,
            },
            WsStream {
                stream,
                deflate: self.deflate,
                max_size: self.max_size,
                fragment: None,
            },
        )
    }
}

/// The write half — owned by the per-connection writer process. Each method sends one
/// message as a single (unfragmented) frame; data messages are deflated when negotiated.
pub(crate) struct WsSink<S> {
    sink: futures_util::stream::SplitSink<FramedConn<S>, RawFrame>,
    deflate: bool,
}

impl<S> WsSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    async fn send_data(&mut self, opcode: Data, payload: Vec<u8>) -> io::Result<()> {
        let (payload, rsv1) = if self.deflate {
            (deflate_message(&payload)?, true)
        } else {
            (payload, false)
        };
        self.sink
            .send(RawFrame {
                fin: true,
                rsv1,
                opcode: OpCode::Data(opcode),
                payload: Bytes::from(payload),
            })
            .await
    }

    pub(crate) async fn send_text(&mut self, payload: Vec<u8>) -> io::Result<()> {
        self.send_data(Data::Text, payload).await
    }

    pub(crate) async fn send_binary(&mut self, payload: Vec<u8>) -> io::Result<()> {
        self.send_data(Data::Binary, payload).await
    }

    async fn send_control(&mut self, opcode: Control, payload: Vec<u8>) -> io::Result<()> {
        // Control frames are never compressed or fragmented (RFC 6455 §5.5).
        self.sink
            .send(RawFrame {
                fin: true,
                rsv1: false,
                opcode: OpCode::Control(opcode),
                payload: Bytes::from(payload),
            })
            .await
    }

    pub(crate) async fn send_ping(&mut self, payload: Vec<u8>) -> io::Result<()> {
        self.send_control(Control::Ping, payload).await
    }

    pub(crate) async fn send_pong(&mut self, payload: Vec<u8>) -> io::Result<()> {
        self.send_control(Control::Pong, payload).await
    }

    pub(crate) async fn send_close(&mut self, code: u16, reason: String) -> io::Result<()> {
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        self.send_control(Control::Close, payload).await
    }
}

/// The read half — owned by the inbound reader loop. [`recv`](Self::recv) assembles
/// fragmented messages, decompresses permessage-deflate, and surfaces control frames.
pub(crate) struct WsStream<S> {
    stream: futures_util::stream::SplitStream<FramedConn<S>>,
    deflate: bool,
    max_size: Option<usize>,
    /// An in-progress fragmented data message: (kind, compressed?, bytes so far).
    fragment: Option<(Data, bool, Vec<u8>)>,
}

impl<S> WsStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// The next complete message, or `None` at end of stream. Control frames are returned as
    /// they arrive (the caller pongs a ping, ends on close); data frames are reassembled.
    pub(crate) async fn recv(&mut self) -> Option<io::Result<WsMessage>> {
        loop {
            let frame = match self.stream.next().await? {
                Ok(frame) => frame,
                Err(e) => return Some(Err(e)),
            };
            match frame.opcode {
                OpCode::Control(control) => {
                    // Control frames must be final and ≤125 bytes, and never compressed.
                    if !frame.fin || frame.payload.len() > 125 || frame.rsv1 {
                        return Some(Err(protocol_error("invalid control frame")));
                    }
                    match control {
                        Control::Ping => return Some(Ok(WsMessage::Ping(frame.payload.to_vec()))),
                        Control::Pong => return Some(Ok(WsMessage::Pong(frame.payload.to_vec()))),
                        Control::Close => {
                            return Some(Ok(WsMessage::Close(parse_close(&frame.payload))))
                        }
                        Control::Reserved(_) => {
                            return Some(Err(protocol_error("reserved control opcode")))
                        }
                    }
                }
                OpCode::Data(Data::Text) | OpCode::Data(Data::Binary) => {
                    if self.fragment.is_some() {
                        return Some(Err(protocol_error("new data frame mid-fragment")));
                    }
                    if frame.rsv1 && !self.deflate {
                        return Some(Err(protocol_error("compressed frame but no deflate")));
                    }
                    let kind = match frame.opcode {
                        OpCode::Data(d) => d,
                        _ => unreachable!(),
                    };
                    if let Some(over) = self.over_limit(frame.payload.len()) {
                        return Some(Err(over));
                    }
                    if frame.fin {
                        return Some(self.finish(kind, frame.rsv1, frame.payload.to_vec()));
                    }
                    self.fragment = Some((kind, frame.rsv1, frame.payload.to_vec()));
                }
                OpCode::Data(Data::Continue) => {
                    let Some((kind, compressed, mut buf)) = self.fragment.take() else {
                        return Some(Err(protocol_error("continuation without a start frame")));
                    };
                    if frame.rsv1 {
                        return Some(Err(protocol_error("rsv1 set on a continuation frame")));
                    }
                    if let Some(over) = self.over_limit(buf.len() + frame.payload.len()) {
                        return Some(Err(over));
                    }
                    buf.extend_from_slice(&frame.payload);
                    if frame.fin {
                        return Some(self.finish(kind, compressed, buf));
                    }
                    self.fragment = Some((kind, compressed, buf));
                }
                OpCode::Data(Data::Reserved(_)) => {
                    return Some(Err(protocol_error("reserved data opcode")))
                }
            }
        }
    }

    /// `Some(error)` if `size` exceeds the configured message-size limit.
    fn over_limit(&self, size: usize) -> Option<io::Error> {
        match self.max_size {
            Some(max) if size > max => Some(protocol_error("message exceeds the size limit")),
            _ => None,
        }
    }

    /// Decompress (if the message was compressed) and tag it Text/Binary.
    fn finish(&self, kind: Data, compressed: bool, data: Vec<u8>) -> io::Result<WsMessage> {
        let data = if compressed {
            inflate_message(&data, self.max_size)?
        } else {
            data
        };
        Ok(match kind {
            Data::Text => WsMessage::Text(data),
            _ => WsMessage::Binary(data),
        })
    }
}

/// Parse a Close payload into its code + reason (empty payload = no code).
fn parse_close(payload: &[u8]) -> Option<(u16, String)> {
    if payload.len() < 2 {
        return None;
    }
    let code = u16::from_be_bytes([payload[0], payload[1]]);
    let reason = String::from_utf8_lossy(&payload[2..]).into_owned();
    Some((code, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::frame::{Frame, FrameHeader};

    #[test]
    fn negotiates_permessage_deflate_only_when_offered() {
        assert_eq!(
            negotiate_permessage_deflate(Some("permessage-deflate")).as_deref(),
            Some("permessage-deflate; client_no_context_takeover; server_no_context_takeover")
        );
        assert!(
            negotiate_permessage_deflate(Some("permessage-deflate; client_max_window_bits"))
                .is_some()
        );
        assert!(negotiate_permessage_deflate(Some("x-some-ext")).is_none());
        assert!(negotiate_permessage_deflate(None).is_none());
    }

    #[test]
    fn deflate_inflate_round_trips() {
        for msg in [
            &b""[..],
            b"hi",
            b"the quick brown fox jumps over the lazy dog",
            &[0u8; 1000][..],
        ] {
            let compressed = deflate_message(msg).unwrap();
            assert_eq!(inflate_message(&compressed, None).unwrap(), msg);
        }
        // A repetitive payload actually shrinks on the wire.
        let big = b"abcabcabc".repeat(100);
        assert!(deflate_message(&big).unwrap().len() < big.len());
    }

    #[test]
    fn inflate_enforces_the_decompressed_size_cap() {
        let compressed = deflate_message(&[b'a'; 10_000]).unwrap();
        assert!(inflate_message(&compressed, Some(100)).is_err());
        assert!(inflate_message(&compressed, Some(10_000)).is_ok());
    }

    #[test]
    fn masking_is_its_own_inverse() {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut data = b"masked payload".to_vec();
        let original = data.clone();
        apply_mask(&mut data, mask);
        assert_ne!(data, original);
        apply_mask(&mut data, mask);
        assert_eq!(data, original);
    }

    /// Encode a masked client frame the way a browser would, so the decoder unmasks it.
    fn masked_client_frame(opcode: OpCode, rsv1: bool, payload: &[u8]) -> BytesMut {
        let header = FrameHeader {
            is_final: true,
            rsv1,
            rsv2: false,
            rsv3: false,
            opcode,
            mask: Some([0xAA, 0xBB, 0xCC, 0xDD]),
        };
        // `Frame::format` applies the mask itself, so hand it the raw payload.
        let frame = Frame::from_payload(header, Bytes::copy_from_slice(payload));
        let mut out = Vec::new();
        frame.format(&mut out).unwrap();
        BytesMut::from(&out[..])
    }

    #[test]
    fn codec_round_trips_a_masked_frame() {
        let mut codec = WsFrameCodec { max_size: None };
        let mut buf = masked_client_frame(OpCode::Data(Data::Binary), false, b"ping");
        let frame = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(&frame.payload[..], b"ping");
        assert!(!frame.rsv1 && frame.fin);
        assert!(buf.is_empty(), "the whole frame was consumed");

        // Encode it back (server side, unmasked) and re-decode.
        let mut out = BytesMut::new();
        codec.encode(frame, &mut out).unwrap();
        let again = codec.decode(&mut out).unwrap().unwrap();
        assert_eq!(&again.payload[..], b"ping");
    }

    #[test]
    fn codec_waits_for_a_partial_frame_and_caps_size() {
        let mut codec = WsFrameCodec { max_size: Some(3) };
        // An oversized frame is rejected before its body is buffered.
        let oversized = masked_client_frame(OpCode::Data(Data::Binary), false, b"toolong");
        assert!(codec.decode(&mut oversized.clone()).is_err());

        let mut codec = WsFrameCodec { max_size: None };
        let full = masked_client_frame(OpCode::Data(Data::Binary), false, b"hello");
        // Feed only the first few bytes → the decoder asks for more (returns None).
        let mut partial = BytesMut::from(&full[..3]);
        assert!(codec.decode(&mut partial).unwrap().is_none());
    }

    /// Drive a `WsConn` over an in-memory duplex, with the far end acting as the client.
    async fn duplex_conn(
        deflate: bool,
    ) -> (
        WsSink<tokio::io::DuplexStream>,
        WsStream<tokio::io::DuplexStream>,
        tokio::io::DuplexStream,
    ) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (sink, stream) = WsConn::new(server_io, deflate, None).split();
        (sink, stream, client_io)
    }

    #[tokio::test]
    async fn assembles_a_fragmented_text_message() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        // Two fragments: a non-final Text "Hel" then a final Continuation "lo".
        let mut first = masked_client_frame(OpCode::Data(Data::Text), false, b"Hel");
        // Clear the FIN bit on the first frame (byte 0 high bit).
        first[0] &= 0x7F;
        client.write_all(&first).await.unwrap();
        let cont = masked_client_frame(OpCode::Data(Data::Continue), false, b"lo");
        client.write_all(&cont).await.unwrap();

        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            WsMessage::Text(b"Hello".to_vec())
        );
    }

    #[tokio::test]
    async fn round_trips_a_compressed_message_both_directions() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sink, mut stream, mut client) = duplex_conn(true).await;

        // Client → server: a masked, RSV1, deflated Binary frame decodes to the original.
        let payload = b"compress me, compress me, compress me".to_vec();
        let deflated = deflate_message(&payload).unwrap();
        let frame = masked_client_frame(OpCode::Data(Data::Binary), true, &deflated);
        client.write_all(&frame).await.unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            WsMessage::Binary(payload.clone())
        );

        // Server → client: the sink deflates + sets RSV1; reading the raw frame back and
        // inflating recovers the message.
        sink.send_text(b"hello from the server".to_vec())
            .await
            .unwrap();
        let mut codec = WsFrameCodec { max_size: None };
        let mut buf = BytesMut::new();
        let out_frame = loop {
            let mut chunk = [0u8; 1024];
            let n = client.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if let Some(frame) = codec.decode(&mut buf).unwrap() {
                break frame;
            }
        };
        assert!(out_frame.rsv1, "server compressed the frame");
        assert_eq!(
            inflate_message(&out_frame.payload, None).unwrap(),
            b"hello from the server"
        );
    }

    #[tokio::test]
    async fn surfaces_a_pong_control_frame() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        let pong = masked_client_frame(OpCode::Control(Control::Pong), false, b"data");
        client.write_all(&pong).await.unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            WsMessage::Pong(b"data".to_vec())
        );
    }

    #[tokio::test]
    async fn rejects_a_reserved_control_opcode() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        let frame = masked_client_frame(OpCode::Control(Control::Reserved(0xB)), false, b"");
        client.write_all(&frame).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "reserved control opcode must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_a_reserved_data_opcode() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        let frame = masked_client_frame(OpCode::Data(Data::Reserved(0x3)), false, b"");
        client.write_all(&frame).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "reserved data opcode must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_new_data_frame_mid_fragment() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        // Start a fragment (non-final Text frame).
        let mut first = masked_client_frame(OpCode::Data(Data::Text), false, b"hello");
        first[0] &= 0x7F; // clear FIN bit
        client.write_all(&first).await.unwrap();
        // Send another non-final data frame before completing the fragment.
        let mut second = masked_client_frame(OpCode::Data(Data::Binary), false, b"world");
        second[0] &= 0x7F;
        client.write_all(&second).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "new data frame mid-fragment must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_compressed_frame_when_deflate_not_negotiated() {
        use tokio::io::AsyncWriteExt;
        // deflate=false: connection did NOT negotiate permessage-deflate.
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        // RSV1=true signals compression; without deflate negotiation this is invalid.
        let frame = masked_client_frame(OpCode::Data(Data::Binary), true, b"data");
        client.write_all(&frame).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "compressed frame without deflate negotiation must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_a_continuation_without_a_start_frame() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        let cont = masked_client_frame(OpCode::Data(Data::Continue), false, b"orphan");
        client.write_all(&cont).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "continuation without a start frame must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_rsv1_on_a_continuation_frame() {
        use tokio::io::AsyncWriteExt;
        // deflate=true so the connection IS negotiated; the initial frame has rsv1=false.
        let (_sink, mut stream, mut client) = duplex_conn(true).await;
        let mut first = masked_client_frame(OpCode::Data(Data::Binary), false, b"hello");
        first[0] &= 0x7F; // clear FIN — this is a non-final, uncompressed start frame
        client.write_all(&first).await.unwrap();
        // Continuation with RSV1 set is invalid — RSV1 belongs only on the first data frame.
        let bad_cont = masked_client_frame(OpCode::Data(Data::Continue), true, b"world");
        client.write_all(&bad_cont).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "RSV1 set on continuation must be a protocol error"
        );
    }

    #[tokio::test]
    async fn rejects_a_non_final_control_frame() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;
        // Control frames (RFC 6455 §5.5) must not be fragmented (fin=0 is invalid).
        let mut ping = masked_client_frame(OpCode::Control(Control::Ping), false, b"hi");
        ping[0] &= 0x7F; // clear the FIN bit
        client.write_all(&ping).await.unwrap();
        assert!(
            stream.recv().await.unwrap().is_err(),
            "non-final control frame must be a protocol error"
        );
    }

    #[tokio::test]
    async fn surfaces_ping_and_close_control_frames() {
        use tokio::io::AsyncWriteExt;
        let (_sink, mut stream, mut client) = duplex_conn(false).await;

        let ping = masked_client_frame(OpCode::Control(Control::Ping), false, b"hi");
        client.write_all(&ping).await.unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            WsMessage::Ping(b"hi".to_vec())
        );

        let mut close_payload = 1000u16.to_be_bytes().to_vec();
        close_payload.extend_from_slice(b"bye");
        let close = masked_client_frame(OpCode::Control(Control::Close), false, &close_payload);
        client.write_all(&close).await.unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            WsMessage::Close(Some((1000, "bye".to_string())))
        );
    }
}
