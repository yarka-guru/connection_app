use crate::tunnel::protocol::{
    build_acknowledge, build_data_message, build_flag_message, build_syn_message, AgentMessage,
    ACKNOWLEDGE, CHANNEL_CLOSED, FLAG_ACK, FLAG_CONNECT_TO_PORT_ERROR, FLAG_DISCONNECT_TO_PORT,
    FLAG_TERMINATE_SESSION, OUTPUT_STREAM_DATA, PAYLOAD_FLAG, PAYLOAD_OUTPUT,
    STREAM_DATA_PAYLOAD_SIZE,
};
use crate::tunnel::smux::{self, SmuxSession};
use crate::tunnel::websocket::{open_data_channel, open_data_channel_with_version};
use byteorder::{BigEndian, ByteOrder};
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// WebSocket ping interval — 30 seconds keeps the connection alive through
/// Linux NAT/firewall conntrack (default idle timeout is often 120s).
const WS_PING_INTERVAL_SECS: u64 = 30;

/// Max retransmission attempts before giving up.
const MAX_RETRANSMIT_ATTEMPTS: u32 = 3000;

/// How often to check for retransmissions (ms).
const RETRANSMIT_CHECK_INTERVAL_MS: u64 = 100;

/// Default retransmission timeout (ms).
const DEFAULT_RETRANSMIT_TIMEOUT_MS: u64 = 200;

/// Default RTT estimate (ms).
const DEFAULT_ROUND_TRIP_TIME_MS: u64 = 100;

/// Clock granularity for Jacobson/Karels RTO calculation (ms).
const CLOCK_GRANULARITY_MS: i64 = 10;

/// Max retransmission timeout (ms).
const MAX_RETRANSMIT_TIMEOUT_MS: u64 = 1000;

/// Max entries in outgoing/incoming buffers before cancelling the session.
/// Prevents unbounded memory growth from a misbehaving agent.
const MAX_BUFFER_SIZE: usize = 10_000;

/// Number of missed pong responses before declaring WebSocket dead.
/// With 30s ping interval, 3 misses = 90s without response.
const WS_PONG_MISS_THRESHOLD: u64 = 3;

/// Send SSM-level keepalive every N ping ticks.
/// WebSocket pings are transport-level — the SSM relay may not count them
/// as session activity. Without SSM-level messages, the session can idle
/// out (default 20 min) even though the WebSocket is alive.
/// 10 ticks × 30s = every 5 minutes.
const SSM_KEEPALIVE_TICKS: u64 = 10;

/// Outgoing message buffer entry.
struct OutgoingEntry {
    message: Vec<u8>,
    sent_at: std::time::Instant,
    retransmit_count: u32,
}

/// Jacobson/Karels RTT estimator — tracks smoothed RTT and RTT variance
/// to compute retransmission timeout, matching the Go session-manager-plugin.
struct RttEstimator {
    /// Smoothed round-trip time (ms).
    srtt: i64,
    /// Round-trip time variation (ms).
    rttvar: i64,
}

impl RttEstimator {
    fn new() -> Self {
        Self {
            srtt: DEFAULT_ROUND_TRIP_TIME_MS as i64,
            rttvar: 0,
        }
    }

    /// Update the estimator with a new RTT sample and return the new RTO.
    /// Formula (RFC 6298 / Jacobson-Karels):
    ///   RTTVAR = (1 - 1/4) * RTTVAR + 1/4 * |SRTT - sample|
    ///   SRTT   = (1 - 1/8) * SRTT   + 1/8 * sample
    ///   RTO    = SRTT + max(G, 4 * RTTVAR)
    fn update(&mut self, sample_ms: i64) -> i64 {
        let diff = (self.srtt - sample_ms).abs();
        self.rttvar = (self.rttvar * 3 / 4) + (diff / 4);
        self.srtt = (self.srtt * 7 / 8) + (sample_ms / 8);
        let rto = self.srtt + (CLOCK_GRANULARITY_MS).max(4 * self.rttvar);
        rto.max(DEFAULT_RETRANSMIT_TIMEOUT_MS as i64)
            .min(MAX_RETRANSMIT_TIMEOUT_MS as i64)
    }
}

/// Native port forwarding session — replaces session-manager-plugin.
pub async fn start_native_port_forwarding(
    stream_url: String,
    token_value: String,
    local_port: u16,
    cancel: tokio_util::sync::CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), String> {
    // Open data channel (WebSocket + handshake)
    let channel = open_data_channel(&stream_url, &token_value).await?;
    log::info!(
        "SSM data channel open, agent version: {}",
        channel.agent_version
    );

    // Extract sequence numbers before moving ws
    let initial_outgoing_seq = channel.outgoing_seq;
    let initial_incoming_seq = channel.expected_incoming_seq;

    // Split the WebSocket into read/write halves
    let (ws_write_half, ws_read_half) = channel.ws.split();

    // Wrap write half in Arc<Mutex> for shared access
    let ws_write = Arc::new(tokio::sync::Mutex::new(ws_write_half));

    // Bind local TCP listeners with SO_REUSEADDR — critical on Linux where
    // TIME_WAIT lasts 60s (vs ~15s on macOS), blocking reconnections.
    // Listen on both IPv4 (127.0.0.1) and IPv6 (::1) so clients that resolve
    // "localhost" to either address can connect (DataGrip on Linux uses IPv6).
    let listener_v4 = {
        let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, local_port));
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .map_err(|e| format!("Failed to create socket: {}", e))?;
        socket
            .set_reuse_address(true)
            .map_err(|e| format!("Failed to set SO_REUSEADDR: {}", e))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set nonblocking: {}", e))?;
        socket
            .bind(&addr.into())
            .map_err(|e| format!("Failed to bind port {}: {}", local_port, e))?;
        socket
            .listen(128)
            .map_err(|e| format!("Failed to listen on port {}: {}", local_port, e))?;
        let std_listener: std::net::TcpListener = socket.into();
        tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| format!("Failed to create async listener: {}", e))?
    };

    // IPv6 listener — best-effort (may fail if IPv6 is disabled on the system)
    let listener_v6: Option<tokio::net::TcpListener> = {
        let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, local_port));
        socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .ok()
        .and_then(|socket| {
            let _ = socket.set_only_v6(true);
            let _ = socket.set_reuse_address(true);
            let _ = socket.set_nonblocking(true);
            socket.bind(&addr.into()).ok()?;
            socket.listen(128).ok()?;
            let std_listener: std::net::TcpListener = socket.into();
            tokio::net::TcpListener::from_std(std_listener).ok()
        })
    };

    if listener_v6.is_some() {
        log::info!(
            "Listening on 127.0.0.1:{} and [::1]:{}",
            local_port,
            local_port
        );
    } else {
        log::info!(
            "Listening on 127.0.0.1:{} (IPv6 not available)",
            local_port
        );
    }

    // Signal that the tunnel is ready for connections
    if let Some(tx) = ready_tx {
        let _ = tx.send(Ok(()));
    }

    // Shared state — sequence numbers continue from where the handshake left off.
    // The handshake consumed some sequence numbers on both sides:
    //   - outgoing: HandshakeResponse used seq 0 → next outgoing = 1
    //   - incoming: HandshakeRequest was seq 0, HandshakeComplete was seq 1 → next expected = 2
    let outgoing_seq = Arc::new(AtomicI64::new(initial_outgoing_seq));
    let expected_incoming_seq = Arc::new(AtomicI64::new(initial_incoming_seq));
    let session_cancel = CancellationToken::new();
    let tcp_connected = Arc::new(AtomicBool::new(false));

    // Pong watchdog: track when we last received a pong to detect dead WebSockets.
    // Stored as seconds since UNIX epoch (AtomicI64 for lock-free access).
    let last_pong_time = Arc::new(AtomicI64::new(epoch_secs()));

    // Outgoing buffer for retransmission
    let outgoing_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, OutgoingEntry>::new()));

    // Incoming out-of-order buffer: seq → (payload_type, payload)
    let incoming_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, (u32, Vec<u8>)>::new()));

    // Channel for passing received data to the TCP write side
    let (tcp_data_tx, mut tcp_data_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);

    // RTT tracking for retransmission timeout (Jacobson/Karels algorithm)
    let rtt_estimator = Arc::new(tokio::sync::Mutex::new(RttEstimator::new()));

    // Send SYN flag — tells the SSM agent "client is ready for port forwarding".
    // The Go session-manager-plugin sends this after handshake; without it the
    // agent may not connect to the remote host (observed on Linux).
    {
        let syn_seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
        let syn_msg = build_syn_message(syn_seq);
        let serialized = syn_msg.serialize();
        {
            let mut ob = outgoing_buffer.lock().await;
            ob.insert(
                syn_seq,
                OutgoingEntry {
                    message: serialized.clone(),
                    sent_at: std::time::Instant::now(),
                    retransmit_count: 0,
                },
            );
        }
        let mut ws = ws_write.lock().await;
        ws.send(Message::Binary(serialized.into()))
            .await
            .map_err(|e| format!("Failed to send SYN: {}", e))?;
        log::info!("Sent SYN flag to SSM agent");
    }

    // --- Task 1: WebSocket ping keepalive + pong watchdog + SSM keepalive ---
    let ws_write_ping = ws_write.clone();
    let cancel_ping = cancel.clone();
    let session_cancel_ping = session_cancel.clone();
    let last_pong_ping = last_pong_time.clone();
    // Pre-build SSM keepalive message (acknowledge with no-op content).
    // This goes through the SSM relay as a real protocol message, resetting
    // the session idle timer. The agent ignores acks for unknown sequences.
    let ssm_keepalive_bytes = AgentMessage::new(
        ACKNOWLEDGE,
        0,
        FLAG_ACK,
        0,
        serde_json::to_vec(&serde_json::json!({
            "AcknowledgedMessageType": "",
            "AcknowledgedMessageId": "",
            "AcknowledgedMessageSequenceNumber": 0,
            "IsSequentialMessage": false,
        }))
        .unwrap_or_default(),
    )
    .serialize();
    let ping_handle = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(WS_PING_INTERVAL_SECS));
        let mut tick_count: u64 = 0;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cancel_ping.cancelled() => break,
                _ = session_cancel_ping.cancelled() => break,
            }
            tick_count += 1;

            // Check if pong responses have stopped (dead WebSocket detection).
            // If no pong received within PONG_MISS_THRESHOLD * PING_INTERVAL,
            // the remote side is unresponsive — cancel session to trigger reconnect.
            let last = last_pong_ping.load(Ordering::Relaxed);
            let stale_secs = epoch_secs() - last;
            if stale_secs > (WS_PING_INTERVAL_SECS * WS_PONG_MISS_THRESHOLD) as i64 {
                log::error!(
                    "No WebSocket pong received for {}s, declaring session dead",
                    stale_secs
                );
                session_cancel_ping.cancel();
                break;
            }

            let mut ws = ws_write_ping.lock().await;
            if ws
                .send(Message::Ping(b"keepalive".to_vec().into()))
                .await
                .is_err()
            {
                break;
            }

            // SSM-level keepalive: send a no-op acknowledge through the SSM relay
            // to reset the session idle timer (default 20 min). WebSocket pings
            // are transport-level and may not count as session activity.
            if tick_count.is_multiple_of(SSM_KEEPALIVE_TICKS) {
                let _ = ws
                    .send(Message::Binary(ssm_keepalive_bytes.clone().into()))
                    .await;
            }
        }
    });

    // --- Task 2: Retransmission scheduler ---
    let outgoing_buffer_rt = outgoing_buffer.clone();
    let ws_write_rt = ws_write.clone();
    let cancel_rt = cancel.clone();
    let session_cancel_rt = session_cancel.clone();
    let rtt_estimator_rt = rtt_estimator.clone();
    let retransmit_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            RETRANSMIT_CHECK_INTERVAL_MS,
        ));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cancel_rt.cancelled() => break,
                _ = session_cancel_rt.cancelled() => break,
            }

            let current_rto = {
                let est = rtt_estimator_rt.lock().await;
                est.srtt + (CLOCK_GRANULARITY_MS).max(4 * est.rttvar)
            };
            let timeout = tokio::time::Duration::from_millis(
                (current_rto.max(DEFAULT_RETRANSMIT_TIMEOUT_MS as i64).min(MAX_RETRANSMIT_TIMEOUT_MS as i64)) as u64,
            );
            let mut buf = outgoing_buffer_rt.lock().await;
            let mut to_resend = vec![];

            for (seq, entry) in buf.iter_mut() {
                if entry.sent_at.elapsed() > timeout {
                    if entry.retransmit_count >= MAX_RETRANSMIT_ATTEMPTS {
                        log::error!("Max retransmissions reached for seq {}", seq);
                        session_cancel_rt.cancel();
                        break;
                    }
                    entry.retransmit_count += 1;
                    entry.sent_at = std::time::Instant::now();
                    to_resend.push(entry.message.clone());
                }
            }
            drop(buf);

            if !to_resend.is_empty() {
                let mut ws = ws_write_rt.lock().await;
                for data in to_resend {
                    if ws.send(Message::Binary(data.into())).await.is_err() {
                        session_cancel_rt.cancel();
                        break;
                    }
                }
            }
        }
    });

    // --- Task 3: WebSocket read loop ---
    let ws_read = Arc::new(tokio::sync::Mutex::new(ws_read_half));
    let ws_read_task = ws_read.clone();
    let ws_write_ack = ws_write.clone();
    let cancel_ws = cancel.clone();
    let session_cancel_ws = session_cancel.clone();
    let expected_seq = expected_incoming_seq.clone();
    let incoming_buf = incoming_buffer.clone();
    let outgoing_buf_ack = outgoing_buffer.clone();
    let rtt_estimator_ack = rtt_estimator.clone();
    let tcp_connected_ws = tcp_connected.clone();
    let last_pong_ws = last_pong_time.clone();

    let ws_read_handle = tokio::spawn(async move {
        let mut ws = ws_read_task.lock().await;
        loop {
            if cancel_ws.is_cancelled() || session_cancel_ws.is_cancelled() {
                break;
            }

            let msg = tokio::select! {
                msg = ws.next() => msg,
                _ = cancel_ws.cancelled() => break,
                _ = session_cancel_ws.cancelled() => break,
            };

            let msg = match msg {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    log::error!("WebSocket read error: {}", e);
                    session_cancel_ws.cancel();
                    break;
                }
                None => {
                    session_cancel_ws.cancel();
                    break;
                }
            };

            match msg {
                Message::Binary(data) => {
                    let agent_msg = match AgentMessage::deserialize(&data) {
                        Ok(m) => m,
                        Err(e) => {
                            log::warn!("Failed to deserialize message: {}", e);
                            continue;
                        }
                    };

                    // Verify payload integrity — drop messages with mismatched SHA-256 digest
                    if !agent_msg.validate_digest(&data) {
                        log::warn!(
                            "Payload digest mismatch, dropping message seq={}",
                            agent_msg.sequence_number
                        );
                        continue;
                    }

                    match agent_msg.message_type.as_str() {
                        OUTPUT_STREAM_DATA => {
                            // Always acknowledge
                            let ack = build_acknowledge(&agent_msg);
                            let mut ws_w = ws_write_ack.lock().await;
                            let _ = ws_w.send(Message::Binary(ack.serialize().into())).await;
                            drop(ws_w);

                            // Track sequence numbers for ALL payload types (the agent
                            // uses a single counter for output, flag, etc.)
                            let seq = agent_msg.sequence_number;
                            let expected = expected_seq.load(Ordering::Relaxed);
                            if seq == expected {
                                // In-order: process and advance
                                match agent_msg.payload_type {
                                    PAYLOAD_OUTPUT => {
                                        if tcp_connected_ws.load(Ordering::Relaxed) {
                                            let _ = tcp_data_tx.send(agent_msg.payload).await;
                                        }
                                    }
                                    PAYLOAD_FLAG => {
                                        if agent_msg.payload.len() >= 4 {
                                            let flag_value =
                                                BigEndian::read_u32(&agent_msg.payload[..4]);
                                            match flag_value {
                                                FLAG_DISCONNECT_TO_PORT => {
                                                    log::info!("Agent disconnected from remote port");
                                                    tcp_connected_ws.store(false, Ordering::Relaxed);
                                                }
                                                FLAG_CONNECT_TO_PORT_ERROR => {
                                                    log::error!(
                                                        "Agent failed to connect to remote port"
                                                    );
                                                    session_cancel_ws.cancel();
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                expected_seq.store(expected + 1, Ordering::Relaxed);

                                // Drain buffered in-order messages
                                let mut next = expected + 1;
                                let mut ibuf = incoming_buf.lock().await;
                                while let Some((pt, data)) = ibuf.remove(&next) {
                                    match pt {
                                        PAYLOAD_OUTPUT => {
                                            if tcp_connected_ws.load(Ordering::Relaxed) {
                                                let _ = tcp_data_tx.send(data).await;
                                            }
                                        }
                                        PAYLOAD_FLAG => {
                                            if data.len() >= 4 {
                                                let flag_value = BigEndian::read_u32(&data[..4]);
                                                match flag_value {
                                                    FLAG_DISCONNECT_TO_PORT => {
                                                        log::info!("Agent disconnected from remote port");
                                                        tcp_connected_ws.store(false, Ordering::Relaxed);
                                                    }
                                                    FLAG_CONNECT_TO_PORT_ERROR => {
                                                        log::error!("Agent failed to connect to remote port");
                                                        session_cancel_ws.cancel();
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    next += 1;
                                }
                                expected_seq.store(next, Ordering::Relaxed);
                            } else if seq > expected {
                                // Out-of-order: buffer with payload type
                                let mut ibuf = incoming_buf.lock().await;
                                if ibuf.len() >= MAX_BUFFER_SIZE {
                                    log::error!("Incoming buffer overflow ({} entries), cancelling session", MAX_BUFFER_SIZE);
                                    session_cancel_ws.cancel();
                                    break;
                                }
                                ibuf.insert(seq, (agent_msg.payload_type, agent_msg.payload));
                            }
                            // seq < expected: duplicate, drop silently
                        }
                        "acknowledge" => {
                            // Process ack: remove from outgoing buffer, update RTT
                            if let Ok(content) =
                                serde_json::from_slice::<serde_json::Value>(&agent_msg.payload)
                                && let Some(seq) = content
                                    .get("AcknowledgedMessageSequenceNumber")
                                    .and_then(|v| v.as_i64())
                            {
                                let mut buf = outgoing_buf_ack.lock().await;
                                if let Some(entry) = buf.remove(&seq) {
                                    // Only use first-transmission samples for RTT
                                    // (Karn's algorithm: skip retransmitted packets)
                                    if entry.retransmit_count == 0 {
                                        let rtt_sample =
                                            entry.sent_at.elapsed().as_millis() as i64;
                                        let mut est = rtt_estimator_ack.lock().await;
                                        est.update(rtt_sample);
                                    }
                                }
                            }
                        }
                        CHANNEL_CLOSED => {
                            log::info!("Channel closed by agent");
                            session_cancel_ws.cancel();
                            break;
                        }
                        _ => {}
                    }
                }
                Message::Pong(_) => {
                    last_pong_ws.store(epoch_secs(), Ordering::Relaxed);
                }
                Message::Close(_) => {
                    session_cancel_ws.cancel();
                    break;
                }
                _ => {} // Ignore ping/text
            }
        }
    });

    // --- Main loop: accept TCP connections ---
    let mut is_first_connection = true;
    loop {
        if cancel.is_cancelled() || session_cancel.is_cancelled() {
            break;
        }

        let tcp_stream = tokio::select! {
            result = listener_v4.accept() => {
                match result {
                    Ok((stream, _addr)) => stream,
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
                    Ok((stream, _addr)) => stream,
                    Err(e) => {
                        log::error!("TCP accept error (IPv6): {}", e);
                        continue;
                    }
                }
            }
            _ = cancel.cancelled() => break,
            _ = session_cancel.cancelled() => break,
        };

        // Disable Nagle's algorithm — critical for database protocols that
        // rely on prompt delivery of small packets (e.g. PostgreSQL 1-byte SSL response).
        let _ = tcp_stream.set_nodelay(true);

        // Enable TCP keepalive so the kernel detects dead connections.
        // Without this, a silently dropped connection (e.g. laptop sleep, network change)
        // leaves the tunnel stuck forever on Linux.
        let sock_ref = socket2::SockRef::from(&tcp_stream);
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(std::time::Duration::from_secs(60))
            .with_interval(std::time::Duration::from_secs(10));
        let _ = sock_ref.set_tcp_keepalive(&keepalive);

        // Drain any stale data left in the channel from the previous TCP connection
        // (response data in-flight when the previous client disconnected).
        while tcp_data_rx.try_recv().is_ok() {}

        // Re-send SYN to tell the SSM agent to reconnect to the remote port.
        // The initial SYN was sent during setup, but after a client disconnect we
        // send FLAG_DISCONNECT_TO_PORT which tears down the agent's remote TCP
        // connection. Without a new SYN, the agent has nothing to forward to.
        if !is_first_connection {
            let syn_seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
            let syn_msg = build_syn_message(syn_seq);
            let serialized = syn_msg.serialize();
            {
                let mut ob = outgoing_buffer.lock().await;
                ob.insert(
                    syn_seq,
                    OutgoingEntry {
                        message: serialized.clone(),
                        sent_at: std::time::Instant::now(),
                        retransmit_count: 0,
                    },
                );
            }
            let mut ws = ws_write.lock().await;
            if ws
                .send(Message::Binary(serialized.into()))
                .await
                .is_err()
            {
                session_cancel.cancel();
                break;
            }
            log::info!("Re-sent SYN for new TCP client connection");
        }
        is_first_connection = false;

        tcp_connected.store(true, Ordering::Relaxed);

        let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

        // Per-connection cancel token — used to stop the write task when the read side disconnects
        let tcp_conn_cancel = tokio_util::sync::CancellationToken::new();

        // TCP write task: drain data from WebSocket reader
        let session_cancel_tw = session_cancel.clone();
        let cancel_tw = cancel.clone();
        let tcp_connected_tw = tcp_connected.clone();
        let tcp_conn_cancel_tw = tcp_conn_cancel.clone();
        let tcp_write_handle = tokio::spawn(async move {
            loop {
                if cancel_tw.is_cancelled() || session_cancel_tw.is_cancelled() {
                    break;
                }
                tokio::select! {
                    data = tcp_data_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                if tcp_write.write_all(&bytes).await.is_err() {
                                    tcp_connected_tw.store(false, Ordering::Relaxed);
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = cancel_tw.cancelled() => break,
                    _ = tcp_conn_cancel_tw.cancelled() => break,
                }
            }
            // Return the receiver so we can reuse it for the next TCP connection
            tcp_data_rx
        });

        // TCP read loop: forward to WebSocket
        let ws_write_tcp = ws_write.clone();
        let outgoing_buffer_tcp = outgoing_buffer.clone();
        let outgoing_seq_tcp = outgoing_seq.clone();
        let session_cancel_tr = session_cancel.clone();
        let cancel_tr = cancel.clone();
        let tcp_connected_tr = tcp_connected.clone();

        loop {
            if cancel_tr.is_cancelled() || session_cancel_tr.is_cancelled() {
                break;
            }

            let mut buf = [0u8; STREAM_DATA_PAYLOAD_SIZE];
            let n = tokio::select! {
                result = tcp_read.read(&mut buf) => {
                    match result {
                        Ok(0) => {
                            tcp_connected_tr.store(false, Ordering::Relaxed);
                            break;
                        }
                        Ok(n) => n,
                        Err(_) => {
                            tcp_connected_tr.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                _ = cancel_tr.cancelled() => break,
            };

            let seq = outgoing_seq_tcp.fetch_add(1, Ordering::Relaxed);
            let msg = build_data_message(&buf[..n], seq);
            let serialized = msg.serialize();

            // Add to outgoing buffer for retransmission tracking
            {
                let mut ob = outgoing_buffer_tcp.lock().await;
                if ob.len() >= MAX_BUFFER_SIZE {
                    log::error!("Outgoing buffer overflow ({} unacked messages), cancelling session", MAX_BUFFER_SIZE);
                    session_cancel_tr.cancel();
                    break;
                }
                ob.insert(
                    seq,
                    OutgoingEntry {
                        message: serialized.clone(),
                        sent_at: std::time::Instant::now(),
                        retransmit_count: 0,
                    },
                );
            }

            let mut ws = ws_write_tcp.lock().await;
            if ws
                .send(Message::Binary(serialized.into()))
                .await
                .is_err()
            {
                session_cancel_tr.cancel();
                break;
            }
        }

        // TCP client disconnected — stop the write task and send DisconnectToPort flag
        tcp_conn_cancel.cancel();

        if !session_cancel.is_cancelled() && !cancel.is_cancelled() {
            let seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
            let flag_msg = build_flag_message(FLAG_DISCONNECT_TO_PORT, seq, false);
            let serialized = flag_msg.serialize();

            // Track in outgoing buffer for retransmission
            {
                let mut ob = outgoing_buffer.lock().await;
                ob.insert(
                    seq,
                    OutgoingEntry {
                        message: serialized.clone(),
                        sent_at: std::time::Instant::now(),
                        retransmit_count: 0,
                    },
                );
            }

            let mut ws = ws_write.lock().await;
            let _ = ws.send(Message::Binary(serialized.into())).await;
        }

        // Wait for TCP write task and recover the receiver
        tcp_data_rx = tcp_write_handle.await.unwrap_or_else(|_| {
            let (_tx, rx) = tokio::sync::mpsc::channel(1024);
            rx
        });

        log::info!("TCP client disconnected, waiting for new connection");
    }

    // --- Cleanup: release resources in correct order ---

    // 1. Drop TCP listeners immediately to release ports.
    //    On Linux, delayed drop can block rebinding for up to 60s (TIME_WAIT).
    drop(listener_v4);
    drop(listener_v6);

    // 2. Signal all spawned tasks to stop, then abort to ensure they release
    //    their Arc<Mutex<ws_write>> clones before we try to send terminate.
    session_cancel.cancel();
    ping_handle.abort();
    retransmit_handle.abort();
    ws_read_handle.abort();
    let _ = ping_handle.await;
    let _ = retransmit_handle.await;
    let _ = ws_read_handle.await;

    // 3. Best-effort: send terminate and close WebSocket.
    //    Always attempt this (even if session_cancel was set by agent) so the
    //    server-side session is cleaned up promptly.
    {
        let seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
        let term_msg = build_flag_message(FLAG_TERMINATE_SESSION, seq, true);
        let serialized = term_msg.serialize();
        let mut ws = ws_write.lock().await;
        let _ = ws.send(Message::Binary(serialized.into())).await;
        let _ = ws.close().await;
    }

    Ok(())
}

/// Multiplexed port forwarding via smux protocol.
///
/// When the SSM agent supports smux (version >= 3.0.196.0), multiple TCP connections
/// can share a single SSM WebSocket tunnel. In this mode, after the SSM handshake,
/// all data payloads are smux-framed, and the local TCP listener accepts multiple
/// concurrent connections, each mapped to a separate smux stream.
pub async fn start_multiplexed_port_forwarding(
    stream_url: String,
    token_value: String,
    local_port: u16,
    cancel: CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), String> {
    // Open data channel with smux-capable client version
    let channel =
        open_data_channel_with_version(&stream_url, &token_value, smux::SMUX_CLIENT_VERSION)
            .await?;
    log::info!(
        "SSM data channel open (multiplexed), agent version: {}",
        channel.agent_version
    );

    // Verify the agent actually supports smux
    if !smux::agent_supports_smux(&channel.agent_version) {
        log::warn!(
            "Agent version {} does not support smux, falling back to basic mode",
            channel.agent_version
        );
        // Fall back: close this channel and re-open in basic mode.
        // This is a rare edge case (agent downgraded after session started).
        drop(channel);
        return start_native_port_forwarding(stream_url, token_value, local_port, cancel, ready_tx)
            .await;
    }

    let initial_outgoing_seq = channel.outgoing_seq;
    let initial_incoming_seq = channel.expected_incoming_seq;

    // Split WebSocket
    let (ws_write_half, ws_read_half) = channel.ws.split();
    let ws_write = Arc::new(tokio::sync::Mutex::new(ws_write_half));

    // Bind TCP listeners (same as basic mode)
    let listener_v4 = {
        let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, local_port));
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .map_err(|e| format!("Failed to create socket: {}", e))?;
        socket
            .set_reuse_address(true)
            .map_err(|e| format!("Failed to set SO_REUSEADDR: {}", e))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set nonblocking: {}", e))?;
        socket
            .bind(&addr.into())
            .map_err(|e| format!("Failed to bind port {}: {}", local_port, e))?;
        socket
            .listen(128)
            .map_err(|e| format!("Failed to listen on port {}: {}", local_port, e))?;
        let std_listener: std::net::TcpListener = socket.into();
        tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| format!("Failed to create async listener: {}", e))?
    };

    let listener_v6: Option<tokio::net::TcpListener> = {
        let addr = std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, local_port));
        socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .ok()
        .and_then(|socket| {
            let _ = socket.set_only_v6(true);
            let _ = socket.set_reuse_address(true);
            let _ = socket.set_nonblocking(true);
            socket.bind(&addr.into()).ok()?;
            socket.listen(128).ok()?;
            let std_listener: std::net::TcpListener = socket.into();
            tokio::net::TcpListener::from_std(std_listener).ok()
        })
    };

    if listener_v6.is_some() {
        log::info!(
            "Multiplexed: listening on 127.0.0.1:{} and [::1]:{}",
            local_port,
            local_port
        );
    } else {
        log::info!(
            "Multiplexed: listening on 127.0.0.1:{} (IPv6 not available)",
            local_port
        );
    }

    // Signal tunnel ready
    if let Some(tx) = ready_tx {
        let _ = tx.send(Ok(()));
    }

    // Shared state
    let outgoing_seq = Arc::new(AtomicI64::new(initial_outgoing_seq));
    let expected_incoming_seq = Arc::new(AtomicI64::new(initial_incoming_seq));
    let session_cancel = CancellationToken::new();
    let last_pong_time = Arc::new(AtomicI64::new(epoch_secs()));

    // Outgoing buffer for SSM-level retransmission
    let outgoing_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, OutgoingEntry>::new()));

    // RTT estimator for retransmission timeout
    let rtt_estimator = Arc::new(tokio::sync::Mutex::new(RttEstimator::new()));

    // Smux frame channel: stream tasks write serialized smux frames here,
    // and the SSM write loop wraps them in input_stream_data messages.
    let (smux_frame_tx, mut smux_frame_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4096);

    // Channel for delivering reassembled SSM payloads to the smux session
    let (ssm_payload_tx, mut ssm_payload_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4096);

    // Create the smux session
    let smux_session = Arc::new(SmuxSession::new(smux_frame_tx.clone(), session_cancel.clone()));

    // Send SYN flag to SSM agent (same as basic mode)
    {
        let syn_seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
        let syn_msg = build_syn_message(syn_seq);
        let serialized = syn_msg.serialize();
        {
            let mut ob = outgoing_buffer.lock().await;
            ob.insert(
                syn_seq,
                OutgoingEntry {
                    message: serialized.clone(),
                    sent_at: std::time::Instant::now(),
                    retransmit_count: 0,
                },
            );
        }
        let mut ws = ws_write.lock().await;
        ws.send(Message::Binary(serialized.into()))
            .await
            .map_err(|e| format!("Failed to send SYN: {}", e))?;
        log::info!("Sent SYN flag to SSM agent (multiplexed mode)");
    }

    // --- Task 1: WebSocket ping keepalive + pong watchdog ---
    let ws_write_ping = ws_write.clone();
    let cancel_ping = cancel.clone();
    let session_cancel_ping = session_cancel.clone();
    let last_pong_ping = last_pong_time.clone();
    let ssm_keepalive_bytes = AgentMessage::new(
        crate::tunnel::protocol::ACKNOWLEDGE,
        0,
        crate::tunnel::protocol::FLAG_ACK,
        0,
        serde_json::to_vec(&serde_json::json!({
            "AcknowledgedMessageType": "",
            "AcknowledgedMessageId": "",
            "AcknowledgedMessageSequenceNumber": 0,
            "IsSequentialMessage": false,
        }))
        .unwrap_or_default(),
    )
    .serialize();
    let ping_handle = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(WS_PING_INTERVAL_SECS));
        let mut tick_count: u64 = 0;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cancel_ping.cancelled() => break,
                _ = session_cancel_ping.cancelled() => break,
            }
            tick_count += 1;

            let last = last_pong_ping.load(Ordering::Relaxed);
            let stale_secs = epoch_secs() - last;
            if stale_secs > (WS_PING_INTERVAL_SECS * WS_PONG_MISS_THRESHOLD) as i64 {
                log::error!(
                    "No WebSocket pong received for {}s, declaring session dead",
                    stale_secs
                );
                session_cancel_ping.cancel();
                break;
            }

            let mut ws = ws_write_ping.lock().await;
            if ws
                .send(Message::Ping(b"keepalive".to_vec().into()))
                .await
                .is_err()
            {
                break;
            }

            if tick_count.is_multiple_of(SSM_KEEPALIVE_TICKS) {
                let _ = ws
                    .send(Message::Binary(ssm_keepalive_bytes.clone().into()))
                    .await;
            }
        }
    });

    // --- Task 2: Retransmission scheduler ---
    let outgoing_buffer_rt = outgoing_buffer.clone();
    let ws_write_rt = ws_write.clone();
    let cancel_rt = cancel.clone();
    let session_cancel_rt = session_cancel.clone();
    let rtt_estimator_rt = rtt_estimator.clone();
    let retransmit_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            RETRANSMIT_CHECK_INTERVAL_MS,
        ));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = cancel_rt.cancelled() => break,
                _ = session_cancel_rt.cancelled() => break,
            }

            let current_rto = {
                let est = rtt_estimator_rt.lock().await;
                est.srtt + (CLOCK_GRANULARITY_MS).max(4 * est.rttvar)
            };
            let timeout = tokio::time::Duration::from_millis(
                (current_rto
                    .max(DEFAULT_RETRANSMIT_TIMEOUT_MS as i64)
                    .min(MAX_RETRANSMIT_TIMEOUT_MS as i64)) as u64,
            );
            let mut buf = outgoing_buffer_rt.lock().await;
            let mut to_resend = vec![];

            for (seq, entry) in buf.iter_mut() {
                if entry.sent_at.elapsed() > timeout {
                    if entry.retransmit_count >= MAX_RETRANSMIT_ATTEMPTS {
                        log::error!("Max retransmissions reached for seq {}", seq);
                        session_cancel_rt.cancel();
                        break;
                    }
                    entry.retransmit_count += 1;
                    entry.sent_at = std::time::Instant::now();
                    to_resend.push(entry.message.clone());
                }
            }
            drop(buf);

            if !to_resend.is_empty() {
                let mut ws = ws_write_rt.lock().await;
                for data in to_resend {
                    if ws.send(Message::Binary(data.into())).await.is_err() {
                        session_cancel_rt.cancel();
                        break;
                    }
                }
            }
        }
    });

    // --- Task 3: WebSocket read loop (SSM layer) ---
    // Reads SSM messages, handles acks/retransmission, and forwards output payloads
    // to the smux session via ssm_payload_tx.
    let ws_read = Arc::new(tokio::sync::Mutex::new(ws_read_half));
    let ws_read_task = ws_read.clone();
    let ws_write_ack = ws_write.clone();
    let cancel_ws = cancel.clone();
    let session_cancel_ws = session_cancel.clone();
    let expected_seq = expected_incoming_seq.clone();
    let incoming_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, (u32, Vec<u8>)>::new()));
    let incoming_buf = incoming_buffer.clone();
    let outgoing_buf_ack = outgoing_buffer.clone();
    let rtt_estimator_ack = rtt_estimator.clone();
    let last_pong_ws = last_pong_time.clone();

    let ws_read_handle = tokio::spawn(async move {
        let mut ws = ws_read_task.lock().await;
        loop {
            if cancel_ws.is_cancelled() || session_cancel_ws.is_cancelled() {
                break;
            }

            let msg = tokio::select! {
                msg = ws.next() => msg,
                _ = cancel_ws.cancelled() => break,
                _ = session_cancel_ws.cancelled() => break,
            };

            let msg = match msg {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    log::error!("WebSocket read error: {}", e);
                    session_cancel_ws.cancel();
                    break;
                }
                None => {
                    session_cancel_ws.cancel();
                    break;
                }
            };

            match msg {
                Message::Binary(data) => {
                    let agent_msg = match AgentMessage::deserialize(&data) {
                        Ok(m) => m,
                        Err(e) => {
                            log::warn!("Failed to deserialize message: {}", e);
                            continue;
                        }
                    };

                    if !agent_msg.validate_digest(&data) {
                        log::warn!(
                            "Payload digest mismatch, dropping message seq={}",
                            agent_msg.sequence_number
                        );
                        continue;
                    }

                    match agent_msg.message_type.as_str() {
                        OUTPUT_STREAM_DATA => {
                            // Always acknowledge
                            let ack = build_acknowledge(&agent_msg);
                            let mut ws_w = ws_write_ack.lock().await;
                            let _ = ws_w.send(Message::Binary(ack.serialize().into())).await;
                            drop(ws_w);

                            let seq = agent_msg.sequence_number;
                            let expected = expected_seq.load(Ordering::Relaxed);
                            if seq == expected {
                                // In-order: forward payload to smux session
                                if agent_msg.payload_type == PAYLOAD_OUTPUT {
                                    let _ = ssm_payload_tx.send(agent_msg.payload).await;
                                } else if agent_msg.payload_type == PAYLOAD_FLAG {
                                    // Handle SSM-level flags (shouldn't happen much in mux mode)
                                    if agent_msg.payload.len() >= 4 {
                                        let flag_value =
                                            BigEndian::read_u32(&agent_msg.payload[..4]);
                                        if flag_value == FLAG_CONNECT_TO_PORT_ERROR {
                                            log::error!(
                                                "Agent failed to connect to remote port"
                                            );
                                            session_cancel_ws.cancel();
                                        }
                                    }
                                }
                                expected_seq.store(expected + 1, Ordering::Relaxed);

                                // Drain buffered in-order messages
                                let mut next = expected + 1;
                                let mut ibuf = incoming_buf.lock().await;
                                while let Some((pt, payload_data)) = ibuf.remove(&next) {
                                    if pt == PAYLOAD_OUTPUT {
                                        let _ = ssm_payload_tx.send(payload_data).await;
                                    }
                                    next += 1;
                                }
                                expected_seq.store(next, Ordering::Relaxed);
                            } else if seq > expected {
                                let mut ibuf = incoming_buf.lock().await;
                                if ibuf.len() >= MAX_BUFFER_SIZE {
                                    log::error!("Incoming buffer overflow, cancelling session");
                                    session_cancel_ws.cancel();
                                    break;
                                }
                                ibuf.insert(seq, (agent_msg.payload_type, agent_msg.payload));
                            }
                        }
                        "acknowledge" => {
                            if let Ok(content) =
                                serde_json::from_slice::<serde_json::Value>(&agent_msg.payload)
                                && let Some(seq) = content
                                    .get("AcknowledgedMessageSequenceNumber")
                                    .and_then(|v| v.as_i64())
                            {
                                let mut buf = outgoing_buf_ack.lock().await;
                                if let Some(entry) = buf.remove(&seq)
                                    && entry.retransmit_count == 0 {
                                        let rtt_sample =
                                            entry.sent_at.elapsed().as_millis() as i64;
                                        let mut est = rtt_estimator_ack.lock().await;
                                        est.update(rtt_sample);
                                    }
                            }
                        }
                        CHANNEL_CLOSED => {
                            log::info!("Channel closed by agent");
                            session_cancel_ws.cancel();
                            break;
                        }
                        _ => {}
                    }
                }
                Message::Pong(_) => {
                    last_pong_ws.store(epoch_secs(), Ordering::Relaxed);
                }
                Message::Close(_) => {
                    session_cancel_ws.cancel();
                    break;
                }
                _ => {}
            }
        }
    });

    // --- Task 4: Smux frame write loop ---
    // Reads serialized smux frames from the channel and wraps them in SSM input_stream_data.
    let ws_write_smux = ws_write.clone();
    let outgoing_buffer_smux = outgoing_buffer.clone();
    let outgoing_seq_smux = outgoing_seq.clone();
    let cancel_smux_write = cancel.clone();
    let session_cancel_smux_write = session_cancel.clone();
    let smux_write_handle = tokio::spawn(async move {
        loop {
            let frame_data = tokio::select! {
                data = smux_frame_rx.recv() => {
                    match data {
                        Some(d) => d,
                        None => break,
                    }
                }
                _ = cancel_smux_write.cancelled() => break,
                _ = session_cancel_smux_write.cancelled() => break,
            };

            // Wrap smux frame bytes in an SSM input_stream_data message
            let seq = outgoing_seq_smux.fetch_add(1, Ordering::Relaxed);
            let msg = build_data_message(&frame_data, seq);
            let serialized = msg.serialize();

            {
                let mut ob = outgoing_buffer_smux.lock().await;
                if ob.len() >= MAX_BUFFER_SIZE {
                    log::error!("Outgoing buffer overflow, cancelling session");
                    session_cancel_smux_write.cancel();
                    break;
                }
                ob.insert(
                    seq,
                    OutgoingEntry {
                        message: serialized.clone(),
                        sent_at: std::time::Instant::now(),
                        retransmit_count: 0,
                    },
                );
            }

            let mut ws = ws_write_smux.lock().await;
            if ws
                .send(Message::Binary(serialized.into()))
                .await
                .is_err()
            {
                session_cancel_smux_write.cancel();
                break;
            }
        }
    });

    // --- Task 5: Smux payload dispatch loop ---
    // Reads reassembled SSM payloads and dispatches them to the smux session.
    let smux_session_dispatch = smux_session.clone();
    let cancel_dispatch = cancel.clone();
    let session_cancel_dispatch = session_cancel.clone();
    let dispatch_handle = tokio::spawn(async move {
        loop {
            let payload = tokio::select! {
                data = ssm_payload_rx.recv() => {
                    match data {
                        Some(d) => d,
                        None => break,
                    }
                }
                _ = cancel_dispatch.cancelled() => break,
                _ = session_cancel_dispatch.cancelled() => break,
            };

            smux_session_dispatch.handle_incoming_data(&payload).await;
        }
    });

    // --- Task 6: Smux keepalive ---
    let smux_session_keepalive = smux_session.clone();
    let keepalive_handle = tokio::spawn(async move {
        smux_session_keepalive.run_keepalive().await;
    });

    // --- Task 7: Multiplexed TCP listener ---
    let smux_session_listener = smux_session.clone();
    let cancel_listener = cancel.clone();
    let session_cancel_listener = session_cancel.clone();
    let listener_cancel = CancellationToken::new();
    let listener_cancel_inner = listener_cancel.clone();
    let listener_handle = tokio::spawn(async move {
        let combined_cancel = CancellationToken::new();
        let cc1 = combined_cancel.clone();
        let cc2 = combined_cancel.clone();

        // Monitor both parent cancellation tokens
        let monitor1 = tokio::spawn(async move {
            cancel_listener.cancelled().await;
            cc1.cancel();
        });
        let monitor2 = tokio::spawn(async move {
            session_cancel_listener.cancelled().await;
            cc2.cancel();
        });

        smux::run_multiplexed_listener(
            smux_session_listener,
            listener_v4,
            listener_v6,
            combined_cancel,
        )
        .await;

        listener_cancel_inner.cancel();
        monitor1.abort();
        monitor2.abort();
    });

    // Wait for session to end (cancel or session_cancel)
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = session_cancel.cancelled() => {}
        _ = listener_cancel.cancelled() => {}
    }

    // --- Cleanup ---
    session_cancel.cancel();
    ping_handle.abort();
    retransmit_handle.abort();
    ws_read_handle.abort();
    smux_write_handle.abort();
    dispatch_handle.abort();
    keepalive_handle.abort();
    listener_handle.abort();

    let _ = ping_handle.await;
    let _ = retransmit_handle.await;
    let _ = ws_read_handle.await;
    let _ = smux_write_handle.await;
    let _ = dispatch_handle.await;
    let _ = keepalive_handle.await;
    let _ = listener_handle.await;

    // Best-effort: send terminate and close WebSocket
    {
        let seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
        let term_msg = build_flag_message(FLAG_TERMINATE_SESSION, seq, true);
        let serialized = term_msg.serialize();
        let mut ws = ws_write.lock().await;
        let _ = ws.send(Message::Binary(serialized.into())).await;
        let _ = ws.close().await;
    }

    Ok(())
}

/// Current time as seconds since UNIX epoch (for pong watchdog).
fn epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
