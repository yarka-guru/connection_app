//! Smux (Simple Multiplexing) protocol implementation for SSM WebSocket tunnels.
//!
//! When the SSM agent version >= "3.0.196.0" and the client reports version >= "1.1.70.0",
//! the SSM agent enables multiplexed mode. In this mode, all data payloads after the
//! handshake are wrapped in smux frames, allowing multiple TCP connections to share
//! a single SSM WebSocket tunnel.
//!
//! Protocol reference: AWS session-manager-plugin smux implementation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

// --- Smux protocol constants ---

/// Smux protocol version — must match the SSM agent's smux version.
/// AWS SSM agent uses smux v1 (version byte = 1).
const SMUX_VERSION: u8 = 1;

/// Smux frame header size in bytes.
const SMUX_HEADER_SIZE: usize = 8;

/// Maximum frame payload size.
const MAX_FRAME_SIZE: usize = 65535;

/// Keepalive interval in seconds.
///
/// MUST be well below the SSM agent's smux KeepAliveTimeout (30s, xtaci/smux
/// DefaultConfig). The agent closes the whole session (`channel_closed`) if it
/// receives no frame — data or NOP — for 30s. Sending every 30s created a
/// millisecond-level race against that deadline: whenever a NOP arrived late
/// (network jitter, scheduling), the agent hung up ~30s after the last frame,
/// producing constant tunnel flapping on idle connections. 10s matches the
/// official session-manager-plugin (xtaci/smux KeepAliveInterval), a 3x margin.
const KEEPALIVE_INTERVAL_SECS: u64 = 10;

// --- Smux commands ---

/// SYN: open a new stream.
const CMD_SYN: u8 = 0;
/// FIN: close a stream.
const CMD_FIN: u8 = 1;
/// PSH: push data on a stream.
const CMD_PSH: u8 = 2;
/// NOP: keepalive (no-op).
const CMD_NOP: u8 = 3;
/// UPD: update receive window — smux protocol v2 ONLY. The SSM agent and the
/// official plugin both run smux v1 (xtaci/smux DefaultConfig); xtaci's
/// recvLoop returns ErrInvalidProtocol and closes the entire mux session if a
/// v1 peer receives this command. We must parse it defensively but NEVER send it.
const CMD_UPD: u8 = 4;

/// A parsed smux frame.
#[derive(Debug, Clone)]
pub struct SmuxFrame {
    pub version: u8,
    pub cmd: u8,
    pub length: u16,
    pub stream_id: u32,
    pub payload: Vec<u8>,
}

impl SmuxFrame {
    /// Create a new smux frame.
    fn new(cmd: u8, stream_id: u32, payload: Vec<u8>) -> Self {
        Self {
            version: SMUX_VERSION,
            cmd,
            length: payload.len() as u16,
            stream_id,
            payload,
        }
    }

    /// Serialize the frame to bytes for transmission.
    /// Uses little-endian for length and stream_id, matching xtaci/smux (used by AWS SSM agent).
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(SMUX_HEADER_SIZE + self.payload.len());
        buf.push(self.version);
        buf.push(self.cmd);
        buf.extend_from_slice(&self.length.to_le_bytes());
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Deserialize a frame from a byte buffer.
    /// Returns the frame and the number of bytes consumed, or None if insufficient data.
    /// Uses little-endian for length and stream_id, matching xtaci/smux (used by AWS SSM agent).
    pub fn deserialize(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < SMUX_HEADER_SIZE {
            return None;
        }

        let version = data[0];
        let cmd = data[1];
        let length = u16::from_le_bytes([data[2], data[3]]);
        let stream_id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

        let total_len = SMUX_HEADER_SIZE + length as usize;
        if data.len() < total_len {
            return None;
        }

        let payload = data[SMUX_HEADER_SIZE..total_len].to_vec();

        Some((
            Self {
                version,
                cmd,
                length,
                stream_id,
                payload,
            },
            total_len,
        ))
    }

    /// Create a NOP (keepalive) frame.
    fn nop() -> Self {
        Self::new(CMD_NOP, 0, vec![])
    }

    /// Create a FIN frame to close a stream.
    fn fin(stream_id: u32) -> Self {
        Self::new(CMD_FIN, stream_id, vec![])
    }

    /// Create a PSH (data push) frame.
    fn psh(stream_id: u32, data: Vec<u8>) -> Self {
        Self::new(CMD_PSH, stream_id, data)
    }
}

/// Per-stream state for data routing.
///
/// smux v1 has no wire-level per-stream flow control (cmdUPD is v2-only).
/// Backpressure comes from the bounded channels along the data path instead:
/// receive side via `data_tx`, send side via the session's `frame_tx`.
struct SmuxStream {
    /// Channel to send received data to the TCP write side.
    data_tx: mpsc::Sender<Vec<u8>>,
}

/// Outbound frame sender — shared by stream tasks to write smux frames.
type FrameTx = mpsc::Sender<Vec<u8>>;

/// The smux session multiplexer.
///
/// After the SSM handshake, this takes over the data channel:
/// - Incoming SSM payloads are parsed as smux frames and routed to streams
/// - Outgoing TCP data is wrapped in smux frames and sent via the SSM data channel
/// - A local TCP listener accepts multiple connections, each mapped to a smux stream
pub struct SmuxSession {
    /// Channel to send serialized smux frame bytes to the SSM write loop.
    frame_tx: FrameTx,
    /// Active streams keyed by stream ID.
    streams: Arc<Mutex<HashMap<u32, SmuxStream>>>,
    /// Cancellation token for the entire session.
    cancel: CancellationToken,
    /// Buffer for incomplete smux frames that span multiple SSM payloads.
    reassembly_buf: Mutex<Vec<u8>>,
}

impl SmuxSession {
    /// Create a new smux session.
    ///
    /// `frame_tx` is used to send serialized smux frame bytes out to the SSM data channel.
    /// The caller is responsible for reading from the corresponding `frame_rx` and wrapping
    /// the bytes in SSM `input_stream_data` messages.
    pub fn new(frame_tx: FrameTx, cancel: CancellationToken) -> Self {
        Self {
            frame_tx,
            streams: Arc::new(Mutex::new(HashMap::new())),
            cancel,
            reassembly_buf: Mutex::new(Vec::new()),
        }
    }

    /// Handle an incoming SSM data payload that contains smux frames.
    ///
    /// A single SSM payload may contain multiple concatenated smux frames.
    /// This parses them all and dispatches to the appropriate stream.
    pub async fn handle_incoming_data(&self, incoming: &[u8]) {
        // Prepend any leftover bytes from the previous SSM payload
        let mut buf = self.reassembly_buf.lock().await;
        let owned;
        let mut data: &[u8] = if buf.is_empty() {
            incoming
        } else {
            buf.extend_from_slice(incoming);
            owned = std::mem::take(&mut *buf);
            &owned
        };

        while !data.is_empty() {
            let (frame, consumed) = match SmuxFrame::deserialize(data) {
                Some(f) => f,
                None => {
                    // Incomplete frame — buffer for next SSM payload
                    *buf = data.to_vec();
                    break;
                }
            };
            data = &data[consumed..];

            match frame.cmd {
                CMD_SYN => {
                    log::info!("Smux SYN received for stream {}", frame.stream_id);
                    // Server opened a new stream — this happens when a new TCP connection
                    // arrives at the remote end. We don't initiate streams from client side
                    // in port forwarding mode; the server SYN is acknowledged implicitly
                    // by our accepting it into the streams map. The actual TCP connection
                    // on the local side is handled by the TCP listener loop in native.rs.
                    //
                    // In SSM port forwarding, the flow is:
                    // 1. Client accepts TCP connection locally
                    // 2. Client creates a stream and sends SYN
                    // 3. Server sends data via PSH on that stream
                    //
                    // OR (server-initiated, observed in some agent versions):
                    // 1. Server sends SYN with new stream ID
                    // 2. Client should associate this with an incoming TCP connection
                    //
                    // For now, we create a placeholder stream that buffers data.
                    // The TCP accept loop will adopt server-initiated streams.
                    let (data_tx, _data_rx) = mpsc::channel(1024);
                    let mut streams = self.streams.lock().await;
                    streams.insert(frame.stream_id, SmuxStream { data_tx });
                }
                CMD_FIN => {
                    log::info!("Smux FIN received for stream {}", frame.stream_id);
                    let mut streams = self.streams.lock().await;
                    streams.remove(&frame.stream_id);
                }
                CMD_PSH => {
                    let stream_id = frame.stream_id;
                    // Clone the sender out of the map so the lock is not held
                    // across the send await — a slow stream must not block
                    // dispatch (SYN/FIN/other streams) behind a full channel.
                    let tx = {
                        let streams = self.streams.lock().await;
                        streams.get(&stream_id).map(|s| s.data_tx.clone())
                    };
                    match tx {
                        Some(tx) => {
                            let _ = tx.send(frame.payload).await;
                        }
                        None => {
                            log::warn!("Received PSH for unknown stream {}", stream_id);
                            // Send FIN for unknown streams so the remote cleans up
                            let fin = SmuxFrame::fin(stream_id).serialize();
                            let _ = self.frame_tx.send(fin).await;
                        }
                    }
                }
                CMD_NOP => {
                    // Keepalive received — timestamp already updated above.
                    log::trace!("Smux NOP keepalive received");
                }
                CMD_UPD => {
                    // v2-only window update — we run v1 (matching the agent),
                    // where there is no wire-level flow control. A v1 peer
                    // never sends this; tolerate and ignore it.
                    log::warn!(
                        "Ignoring unexpected cmdUPD frame on v1 session (stream {})",
                        frame.stream_id
                    );
                }
                _ => {
                    log::warn!("Unknown smux command: {}", frame.cmd);
                }
            }
        }
    }

    /// Open a new client-initiated stream for a local TCP connection.
    ///
    /// Returns the stream ID and a receiver for data from the remote end.
    pub async fn open_stream(&self, stream_id_counter: &AtomicU32) -> (u32, mpsc::Receiver<Vec<u8>>) {
        let stream_id = stream_id_counter.fetch_add(2, Ordering::Relaxed);
        let (data_tx, data_rx) = mpsc::channel::<Vec<u8>>(1024);

        // Send SYN frame to open the stream
        let syn = SmuxFrame::new(CMD_SYN, stream_id, vec![]).serialize();
        let _ = self.frame_tx.send(syn).await;

        // Register the stream
        let mut streams = self.streams.lock().await;
        streams.insert(stream_id, SmuxStream { data_tx });

        (stream_id, data_rx)
    }

    /// Close a stream by sending FIN and removing it from the map.
    pub async fn close_stream(&self, stream_id: u32) {
        let fin = SmuxFrame::fin(stream_id).serialize();
        let _ = self.frame_tx.send(fin).await;
        let mut streams = self.streams.lock().await;
        streams.remove(&stream_id);
    }

    /// Send data on a stream, split into MAX_FRAME_SIZE chunks.
    ///
    /// smux v1 has no wire-level send window: the agent never sends cmdUPD,
    /// so waiting for window refills can never complete (uploads used to
    /// stop at 4 MiB for exactly that reason). Backpressure is provided by
    /// the bounded `frame_tx` channel and, transitively, the SSM data channel.
    pub async fn send_data(&self, stream_id: u32, data: &[u8]) -> Result<(), String> {
        {
            let streams = self.streams.lock().await;
            if !streams.contains_key(&stream_id) {
                return Err(format!("Stream {} not found", stream_id));
            }
        }
        for chunk in data.chunks(MAX_FRAME_SIZE) {
            let psh = SmuxFrame::psh(stream_id, chunk.to_vec()).serialize();
            tokio::select! {
                result = self.frame_tx.send(psh) => {
                    result.map_err(|_| "Frame channel closed".to_string())?;
                }
                _ = self.cancel.cancelled() => return Err("Session cancelled".to_string()),
            }
        }
        Ok(())
    }

    /// Run the keepalive loop — sends NOP frames periodically.
    ///
    /// There is deliberately NO receive-timeout here: modern SSM agents do not
    /// send smux NOPs and our outgoing NOPs are not echoed, so an idle tunnel
    /// legitimately receives no smux frames. Cancelling on that (as this loop
    /// used to after 60s) killed every idle session. Dead-tunnel detection is
    /// handled at the right layer by native.rs: the WebSocket pong watchdog
    /// and the SSM activity watchdog (fed by acknowledge messages for these
    /// very NOPs).
    pub async fn run_keepalive(&self) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS));

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.cancel.cancelled() => break,
            }

            // Send NOP keepalive
            let nop = SmuxFrame::nop().serialize();
            log::debug!("Smux NOP keepalive queued ({} bytes)", nop.len());
            if self.frame_tx.send(nop).await.is_err() {
                log::debug!("Smux NOP keepalive: frame channel closed, stopping keepalive");
                break;
            }
        }
    }
}

/// Run a multiplexed port forwarding session.
///
/// This replaces the single-connection TCP relay loop in `native.rs` for multiplexed mode.
/// It accepts multiple TCP connections on the local port and maps each to a smux stream,
/// all sharing the same SSM WebSocket tunnel.
///
/// # Arguments
/// * `smux_session` - The smux session multiplexer
/// * `listener_v4` - IPv4 TCP listener
/// * `listener_v6` - Optional IPv6 TCP listener
/// * `cancel` - Cancellation token
pub async fn run_multiplexed_listener(
    smux_session: Arc<SmuxSession>,
    listener_v4: tokio::net::TcpListener,
    listener_v6: Option<tokio::net::TcpListener>,
    cancel: CancellationToken,
) {
    // Client-initiated stream IDs are odd numbers (1, 3, 5, ...).
    // Server-initiated stream IDs are even numbers (0, 2, 4, ...).
    let stream_id_counter = Arc::new(AtomicU32::new(1));

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let tcp_stream = tokio::select! {
            result = listener_v4.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        log::info!("Multiplexed: TCP connection from {} (IPv4)", addr);
                        stream
                    }
                    Err(e) => {
                        log::error!("TCP accept error (IPv4): {}", e);
                        continue;
                    }
                }
            }
            result = async {
                match &listener_v6 {
                    Some(l) => l.accept().await,
                    None => std::future::pending().await,
                }
            } => {
                match result {
                    Ok((stream, addr)) => {
                        log::info!("Multiplexed: TCP connection from {} (IPv6)", addr);
                        stream
                    }
                    Err(e) => {
                        log::error!("TCP accept error (IPv6): {}", e);
                        continue;
                    }
                }
            }
            _ = cancel.cancelled() => break,
        };

        // Disable Nagle's algorithm
        let _ = tcp_stream.set_nodelay(true);

        // Enable TCP keepalive
        let sock_ref = socket2::SockRef::from(&tcp_stream);
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(std::time::Duration::from_secs(60))
            .with_interval(std::time::Duration::from_secs(10));
        let _ = sock_ref.set_tcp_keepalive(&keepalive);

        // Open a new smux stream for this TCP connection
        let session = smux_session.clone();
        let cancel_child = cancel.clone();
        let counter = stream_id_counter.clone();

        tokio::spawn(async move {
            let (stream_id, mut data_rx) = session.open_stream(&counter).await;
            log::info!("Opened smux stream {} for TCP connection", stream_id);

            let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

            // Task: remote -> TCP (write data from smux stream to TCP)
            let session_write = session.clone();
            let cancel_write = cancel_child.clone();
            let write_handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        data = data_rx.recv() => {
                            match data {
                                Some(bytes) => {
                                    if tcp_write.write_all(&bytes).await.is_err() {
                                        break;
                                    }
                                }
                                None => break, // Stream closed
                            }
                        }
                        _ = cancel_write.cancelled() => break,
                    }
                }
                let _ = session_write; // keep session alive
            });

            // TCP -> remote (read from TCP and send via smux stream)
            let mut buf = vec![0u8; MAX_FRAME_SIZE];
            loop {
                if cancel_child.is_cancelled() {
                    break;
                }

                let n = tokio::select! {
                    result = tcp_read.read(&mut buf) => {
                        match result {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        }
                    }
                    _ = cancel_child.cancelled() => break,
                };

                if let Err(e) = session.send_data(stream_id, &buf[..n]).await {
                    log::warn!("Failed to send data on smux stream {}: {}", stream_id, e);
                    break;
                }
            }

            // Clean up: close the smux stream
            write_handle.abort();
            let _ = write_handle.await;
            session.close_stream(stream_id).await;
            log::info!("Closed smux stream {}", stream_id);
        });
    }
}

/// Check if the agent version supports smux multiplexing.
/// Smux is enabled when agent version >= "3.0.196.0".
pub fn agent_supports_smux(agent_version: &str) -> bool {
    let parts: Vec<u32> = agent_version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();

    if parts.len() < 3 {
        return false;
    }

    // Compare: 3.0.196.0
    match parts[0].cmp(&3) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match parts[1].cmp(&0) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => parts[2] >= 196,
        },
    }
}

/// The client version to report when smux multiplexing is desired.
pub const SMUX_CLIENT_VERSION: &str = "1.2.0.0";

/// The client version to report for basic (non-multiplexed) mode.
pub const BASIC_CLIENT_VERSION: &str = "1.0.0.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_serialize_deserialize_roundtrip() {
        let frame = SmuxFrame::new(CMD_PSH, 42, b"hello smux".to_vec());
        let bytes = frame.serialize();

        let (decoded, consumed) = SmuxFrame::deserialize(&bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded.version, SMUX_VERSION);
        assert_eq!(decoded.cmd, CMD_PSH);
        assert_eq!(decoded.stream_id, 42);
        assert_eq!(decoded.length, 10);
        assert_eq!(decoded.payload, b"hello smux");
    }

    #[test]
    fn frame_header_size() {
        let frame = SmuxFrame::new(CMD_NOP, 0, vec![]);
        let bytes = frame.serialize();
        assert_eq!(bytes.len(), SMUX_HEADER_SIZE);
    }

    #[test]
    fn frame_nop() {
        let frame = SmuxFrame::nop();
        assert_eq!(frame.cmd, CMD_NOP);
        assert_eq!(frame.stream_id, 0);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn frame_fin() {
        let frame = SmuxFrame::fin(7);
        assert_eq!(frame.cmd, CMD_FIN);
        assert_eq!(frame.stream_id, 7);
    }

    #[test]
    fn deserialize_incomplete_header() {
        assert!(SmuxFrame::deserialize(&[0u8; 5]).is_none());
    }

    #[test]
    fn deserialize_incomplete_payload() {
        // Header says 10 bytes payload but only 3 are provided
        let mut data = vec![SMUX_VERSION, CMD_PSH];
        data.extend_from_slice(&10u16.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 3]); // Only 3 of 10 payload bytes
        assert!(SmuxFrame::deserialize(&data).is_none());
    }

    #[test]
    fn deserialize_multiple_frames() {
        let frame1 = SmuxFrame::new(CMD_PSH, 1, b"aaa".to_vec());
        let frame2 = SmuxFrame::new(CMD_PSH, 2, b"bbb".to_vec());
        let mut data = frame1.serialize();
        data.extend_from_slice(&frame2.serialize());

        let (f1, consumed1) = SmuxFrame::deserialize(&data).unwrap();
        assert_eq!(f1.stream_id, 1);
        assert_eq!(f1.payload, b"aaa");

        let (f2, consumed2) = SmuxFrame::deserialize(&data[consumed1..]).unwrap();
        assert_eq!(f2.stream_id, 2);
        assert_eq!(f2.payload, b"bbb");
        assert_eq!(consumed1 + consumed2, data.len());
    }

    /// Regression test: a v1 smux session must NEVER emit a cmdUPD frame.
    ///
    /// The SSM agent pairs with the official plugin on smux protocol v1
    /// (smux.DefaultConfig). In xtaci/smux's recvLoop, cmdUPD is v2-only:
    /// receiving it on a v1 session returns ErrInvalidProtocol and closes the
    /// entire mux session — killing every TCP connection in the tunnel. This
    /// manifested as bulk transfers (pg_dump, large SELECTs) dying at ~2 MiB
    /// (the old UPD threshold).
    #[tokio::test]
    async fn v1_session_never_emits_upd_after_large_receive() {
        let (frame_tx, mut frame_rx) = mpsc::channel(4096);
        let session = SmuxSession::new(frame_tx, CancellationToken::new());
        let counter = AtomicU32::new(1);
        let (stream_id, mut data_rx) = session.open_stream(&counter).await;

        // Drain the SYN emitted by open_stream
        let syn_bytes = frame_rx.recv().await.expect("SYN frame");
        let (syn, _) = SmuxFrame::deserialize(&syn_bytes).expect("valid SYN");
        assert_eq!(syn.cmd, CMD_SYN);

        // Feed 3 MiB of PSH data — well past the historical 2 MiB UPD threshold
        let payload = vec![0u8; 60_000];
        let mut fed: usize = 0;
        while fed < 3 * 1024 * 1024 {
            let frame = SmuxFrame::psh(stream_id, payload.clone()).serialize();
            session.handle_incoming_data(&frame).await;
            while data_rx.try_recv().is_ok() {}
            fed += payload.len();
        }

        // No frame the client emitted may be a cmdUPD
        while let Ok(frame_bytes) = frame_rx.try_recv() {
            let (frame, _) = SmuxFrame::deserialize(&frame_bytes).expect("valid frame");
            assert_ne!(
                frame.cmd, CMD_UPD,
                "v1 session emitted cmdUPD — the agent's recvLoop treats this as \
                 ErrInvalidProtocol and kills the whole mux session"
            );
        }
    }

    /// Regression test: uploads larger than the (removed) 4 MiB pseudo-window
    /// must not hang. A v1 agent never sends window updates, so any send-side
    /// wait on a window refill blocks forever.
    #[tokio::test]
    async fn send_data_does_not_hang_beyond_initial_window() {
        let (frame_tx, mut frame_rx) = mpsc::channel(4096);
        let session = SmuxSession::new(frame_tx, CancellationToken::new());
        let counter = AtomicU32::new(1);
        let (stream_id, _data_rx) = session.open_stream(&counter).await;

        // Drain frames concurrently (bounded channel) and count PSH payload bytes
        let drain = tokio::spawn(async move {
            let mut psh_bytes: usize = 0;
            while let Some(frame_bytes) = frame_rx.recv().await {
                let (frame, _) = SmuxFrame::deserialize(&frame_bytes).expect("valid frame");
                if frame.cmd == CMD_PSH {
                    psh_bytes += frame.payload.len();
                }
            }
            psh_bytes
        });

        // 5 MiB > the old 4 MiB MAX_RECEIVE_BUFFER initial window
        let chunk = vec![0u8; 1024 * 1024];
        let send_all = async {
            for _ in 0..5 {
                session.send_data(stream_id, &chunk).await.expect("send_data");
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), send_all)
            .await
            .expect("send_data hung waiting for a window update that a v1 agent never sends");

        drop(session); // closes frame_tx so the drain task finishes
        let total = drain.await.expect("drain task");
        assert_eq!(total, 5 * 1024 * 1024);
    }

    /// Regression test: an idle tunnel must not self-disconnect.
    ///
    /// Modern SSM agents do not send smux NOP keepalives, and our own outgoing
    /// NOPs are not echoed — so on an idle tunnel no smux frame ever arrives.
    /// The keepalive loop used to cancel the session after 60s without a
    /// received frame, which killed every idle CLI tunnel (the GUI masked it
    /// by silently auto-reconnecting every minute). Tunnel liveness is the job
    /// of the SSM-level watchdogs in native.rs (pong + activity), not smux.
    #[tokio::test]
    async fn idle_session_does_not_self_cancel() {
        let (frame_tx, mut frame_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let session = Arc::new(SmuxSession::new(frame_tx, cancel.clone()));

        let keepalive_session = session.clone();
        let handle = tokio::spawn(async move {
            keepalive_session.run_keepalive().await;
        });

        // The interval's first tick fires immediately — give it time to run
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert!(
            !cancel.is_cancelled(),
            "idle session was self-cancelled by the smux keepalive timeout"
        );
        // A NOP keepalive must still be sent
        let nop_bytes = frame_rx.recv().await.expect("NOP frame");
        let (nop, _) = SmuxFrame::deserialize(&nop_bytes).expect("valid frame");
        assert_eq!(nop.cmd, CMD_NOP);

        cancel.cancel();
        let _ = handle.await;
    }

    #[test]
    fn agent_version_check() {
        assert!(agent_supports_smux("3.0.196.0"));
        assert!(agent_supports_smux("3.0.197.0"));
        assert!(agent_supports_smux("3.1.0.0"));
        assert!(agent_supports_smux("4.0.0.0"));
        assert!(agent_supports_smux("3.1.1511.0"));

        assert!(!agent_supports_smux("3.0.195.0"));
        assert!(!agent_supports_smux("2.9.999.0"));
        assert!(!agent_supports_smux("1.0.0.0"));
        assert!(!agent_supports_smux(""));
        assert!(!agent_supports_smux("invalid"));
    }
}
