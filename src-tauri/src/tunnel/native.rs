use crate::tunnel::protocol::{
    build_acknowledge, build_data_message, build_flag_message, AgentMessage, CHANNEL_CLOSED,
    FLAG_CONNECT_TO_PORT_ERROR, FLAG_DISCONNECT_TO_PORT, FLAG_TERMINATE_SESSION, OUTPUT_STREAM_DATA,
    PAYLOAD_FLAG, PAYLOAD_OUTPUT, STREAM_DATA_PAYLOAD_SIZE,
};
use crate::tunnel::websocket::open_data_channel;
use byteorder::{BigEndian, ByteOrder};
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// WebSocket ping interval (5 minutes, per protocol spec).
const WS_PING_INTERVAL_SECS: u64 = 300;

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

    // Bind local TCP listener
    let listener = TcpListener::bind(format!("127.0.0.1:{}", local_port))
        .await
        .map_err(|e| format!("Failed to bind port {}: {}", local_port, e))?;
    log::info!("Listening on 127.0.0.1:{}", local_port);

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

    // Outgoing buffer for retransmission
    let outgoing_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, OutgoingEntry>::new()));

    // Incoming out-of-order buffer: seq → (payload_type, payload)
    let incoming_buffer = Arc::new(tokio::sync::Mutex::new(BTreeMap::<i64, (u32, Vec<u8>)>::new()));

    // Channel for passing received data to the TCP write side
    let (tcp_data_tx, mut tcp_data_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);

    // RTT tracking for retransmission timeout (Jacobson/Karels algorithm)
    let rtt_estimator = Arc::new(tokio::sync::Mutex::new(RttEstimator::new()));

    // --- Task 1: WebSocket ping keepalive ---
    let ws_write_ping = ws_write.clone();
    let cancel_ping = cancel.clone();
    let session_cancel_ping = session_cancel.clone();
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(WS_PING_INTERVAL_SECS));
        loop {
            interval.tick().await;
            if cancel_ping.is_cancelled() || session_cancel_ping.is_cancelled() {
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
        }
    });

    // --- Task 2: Retransmission scheduler ---
    let outgoing_buffer_rt = outgoing_buffer.clone();
    let ws_write_rt = ws_write.clone();
    let cancel_rt = cancel.clone();
    let session_cancel_rt = session_cancel.clone();
    let rtt_estimator_rt = rtt_estimator.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(
            RETRANSMIT_CHECK_INTERVAL_MS,
        ));
        loop {
            interval.tick().await;
            if cancel_rt.is_cancelled() || session_cancel_rt.is_cancelled() {
                break;
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

    tokio::spawn(async move {
        let mut ws = ws_read_task.lock().await;
        loop {
            if cancel_ws.is_cancelled() || session_cancel_ws.is_cancelled() {
                break;
            }

            let msg = tokio::select! {
                msg = ws.next() => msg,
                _ = cancel_ws.cancelled() => break,
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
                Message::Close(_) => {
                    session_cancel_ws.cancel();
                    break;
                }
                _ => {} // Ignore ping/pong/text
            }
        }
    });

    // --- Main loop: accept TCP connections ---
    loop {
        if cancel.is_cancelled() || session_cancel.is_cancelled() {
            break;
        }

        let tcp_stream = tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => stream,
                    Err(e) => {
                        log::error!("TCP accept error: {}", e);
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

        // Drain any stale data left in the channel from the previous TCP connection
        // (response data in-flight when the previous client disconnected).
        while tcp_data_rx.try_recv().is_ok() {}

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

    // Terminate session
    if !session_cancel.is_cancelled() {
        let seq = outgoing_seq.fetch_add(1, Ordering::Relaxed);
        let term_msg = build_flag_message(FLAG_TERMINATE_SESSION, seq, true);
        let serialized = term_msg.serialize();

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
        let _ = ws.close().await;
    }

    Ok(())
}
