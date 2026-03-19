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
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_util::sync::CancellationToken;

// --- Smux protocol constants ---

/// Smux protocol version — must match the SSM agent's smux version.
/// AWS SSM agent uses smux v1 (version byte = 1).
const SMUX_VERSION: u8 = 1;

/// Smux frame header size in bytes.
const SMUX_HEADER_SIZE: usize = 8;

/// Maximum frame payload size.
const MAX_FRAME_SIZE: usize = 65535;

/// Maximum receive buffer per stream (flow control window).
const MAX_RECEIVE_BUFFER: u32 = 4_194_304; // 4 MB

/// Keepalive interval in seconds.
const KEEPALIVE_INTERVAL_SECS: u64 = 30;

/// Keepalive timeout in seconds — if no response within this, session is dead.
const KEEPALIVE_TIMEOUT_SECS: u64 = 60;

// --- Smux commands ---

/// SYN: open a new stream.
const CMD_SYN: u8 = 0;
/// FIN: close a stream.
const CMD_FIN: u8 = 1;
/// PSH: push data on a stream.
const CMD_PSH: u8 = 2;
/// NOP: keepalive (no-op).
const CMD_NOP: u8 = 3;
/// UPD: update receive window.
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

    /// Create a UPD (window update) frame.
    /// Payload is 4 bytes little-endian: the number of bytes consumed (delta).
    fn upd(stream_id: u32, consumed: u32) -> Self {
        Self::new(CMD_UPD, stream_id, consumed.to_le_bytes().to_vec())
    }

    /// Create a PSH (data push) frame.
    fn psh(stream_id: u32, data: Vec<u8>) -> Self {
        Self::new(CMD_PSH, stream_id, data)
    }
}

/// Per-stream state for flow control and data routing.
struct SmuxStream {
    /// Channel to send received data to the TCP write side.
    data_tx: mpsc::Sender<Vec<u8>>,
    /// How many bytes we've consumed from the remote (for window updates).
    consumed_bytes: u32,
    /// Remote peer's send window remaining bytes.
    remote_window: u32,
    /// Notified when remote window opens up (after receiving UPD).
    window_notify: Arc<Notify>,
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
    /// Timestamp of last received frame (for keepalive timeout).
    last_recv: Arc<std::sync::Mutex<std::time::Instant>>,
    /// Buffer for incomplete smux frames that span multiple SSM payloads.
    reassembly_buf: Mutex<Vec<u8>>,
    /// Counter for tracking consumed bytes per stream (for batched UPD).
    upd_threshold: u32,
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
            last_recv: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
            reassembly_buf: Mutex::new(Vec::new()),
            // Send UPD after consuming ~half the max receive buffer
            upd_threshold: MAX_RECEIVE_BUFFER / 2,
        }
    }

    /// Handle an incoming SSM data payload that contains smux frames.
    ///
    /// A single SSM payload may contain multiple concatenated smux frames.
    /// This parses them all and dispatches to the appropriate stream.
    pub async fn handle_incoming_data(&self, incoming: &[u8]) {
        // Update last-received timestamp for keepalive
        if let Ok(mut t) = self.last_recv.lock() {
            *t = std::time::Instant::now();
        }

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
                    streams.insert(
                        frame.stream_id,
                        SmuxStream {
                            data_tx,
                            consumed_bytes: 0,
                            remote_window: MAX_RECEIVE_BUFFER,
                            window_notify: Arc::new(Notify::new()),
                        },
                    );
                }
                CMD_FIN => {
                    log::info!("Smux FIN received for stream {}", frame.stream_id);
                    let mut streams = self.streams.lock().await;
                    streams.remove(&frame.stream_id);
                }
                CMD_PSH => {
                    let payload_len = frame.payload.len() as u32;
                    let stream_id = frame.stream_id;
                    let streams = self.streams.lock().await;
                    if let Some(stream) = streams.get(&stream_id) {
                        // Send data to the TCP write side
                        let _ = stream.data_tx.send(frame.payload).await;
                    } else {
                        log::warn!("Received PSH for unknown stream {}", stream_id);
                        // Send FIN for unknown streams so the remote cleans up
                        let fin = SmuxFrame::fin(stream_id).serialize();
                        let _ = self.frame_tx.send(fin).await;
                        continue;
                    }
                    // Drop the lock before doing more async work
                    drop(streams);

                    // Track consumed bytes for flow control
                    let mut send_upd = false;
                    let mut consumed_total = 0u32;
                    {
                        let mut streams = self.streams.lock().await;
                        if let Some(stream) = streams.get_mut(&stream_id) {
                            stream.consumed_bytes += payload_len;
                            if stream.consumed_bytes >= self.upd_threshold {
                                consumed_total = stream.consumed_bytes;
                                stream.consumed_bytes = 0;
                                send_upd = true;
                            }
                        }
                    }

                    if send_upd {
                        let upd = SmuxFrame::upd(stream_id, consumed_total).serialize();
                        let _ = self.frame_tx.send(upd).await;
                    }
                }
                CMD_NOP => {
                    // Keepalive received — timestamp already updated above.
                    log::trace!("Smux NOP keepalive received");
                }
                CMD_UPD => {
                    // Remote peer consumed bytes, opening up our send window.
                    if frame.payload.len() >= 4 {
                        let consumed =
                            u32::from_le_bytes([frame.payload[0], frame.payload[1], frame.payload[2], frame.payload[3]]);
                        let mut streams = self.streams.lock().await;
                        if let Some(stream) = streams.get_mut(&frame.stream_id) {
                            stream.remote_window = stream.remote_window.saturating_add(consumed);
                            stream.window_notify.notify_waiters();
                        }
                    }
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
        streams.insert(
            stream_id,
            SmuxStream {
                data_tx,
                consumed_bytes: 0,
                remote_window: MAX_RECEIVE_BUFFER,
                window_notify: Arc::new(Notify::new()),
            },
        );

        (stream_id, data_rx)
    }

    /// Close a stream by sending FIN and removing it from the map.
    pub async fn close_stream(&self, stream_id: u32) {
        let fin = SmuxFrame::fin(stream_id).serialize();
        let _ = self.frame_tx.send(fin).await;
        let mut streams = self.streams.lock().await;
        streams.remove(&stream_id);
    }

    /// Send data on a stream, respecting flow control.
    ///
    /// Splits data into chunks of MAX_FRAME_SIZE and waits for window space.
    pub async fn send_data(&self, stream_id: u32, data: &[u8]) -> Result<(), String> {
        let mut offset = 0;
        while offset < data.len() {
            // Determine chunk size based on remote window
            let (chunk_size, window_notify) = {
                let mut streams = self.streams.lock().await;
                let stream = streams
                    .get_mut(&stream_id)
                    .ok_or_else(|| format!("Stream {} not found", stream_id))?;

                if stream.remote_window == 0 {
                    // Need to wait for window update
                    (0usize, Some(stream.window_notify.clone()))
                } else {
                    let remaining = data.len() - offset;
                    let chunk = remaining
                        .min(MAX_FRAME_SIZE)
                        .min(stream.remote_window as usize);
                    stream.remote_window = stream.remote_window.saturating_sub(chunk as u32);
                    (chunk, None)
                }
            };

            if let Some(notify) = window_notify {
                // Wait for window to open up, with cancellation support
                tokio::select! {
                    _ = notify.notified() => continue,
                    _ = self.cancel.cancelled() => return Err("Session cancelled".to_string()),
                }
            }

            if chunk_size > 0 {
                let chunk = &data[offset..offset + chunk_size];
                let psh = SmuxFrame::psh(stream_id, chunk.to_vec()).serialize();
                self.frame_tx
                    .send(psh)
                    .await
                    .map_err(|_| "Frame channel closed".to_string())?;
                offset += chunk_size;
            }
        }
        Ok(())
    }

    /// Run the keepalive loop — sends NOP frames periodically and checks for timeout.
    pub async fn run_keepalive(&self) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS));

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.cancel.cancelled() => break,
            }

            // Check timeout
            let elapsed = {
                let t = self.last_recv.lock().unwrap_or_else(|e| e.into_inner());
                t.elapsed()
            };
            if elapsed > std::time::Duration::from_secs(KEEPALIVE_TIMEOUT_SECS) {
                log::error!(
                    "Smux keepalive timeout ({}s since last frame)",
                    elapsed.as_secs()
                );
                self.cancel.cancel();
                break;
            }

            // Send NOP keepalive
            let nop = SmuxFrame::nop().serialize();
            if self.frame_tx.send(nop).await.is_err() {
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
    fn frame_upd_payload() {
        let frame = SmuxFrame::upd(3, 1024);
        assert_eq!(frame.cmd, CMD_UPD);
        assert_eq!(frame.stream_id, 3);
        assert_eq!(frame.payload.len(), 4);
        let consumed = u32::from_le_bytes([frame.payload[0], frame.payload[1], frame.payload[2], frame.payload[3]]);
        assert_eq!(consumed, 1024);
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
