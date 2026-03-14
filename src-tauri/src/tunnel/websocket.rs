use crate::tunnel::protocol::{
    build_acknowledge, build_handshake_response_with_version, AgentMessage,
    HandshakeCompletePayload, HandshakeRequestPayload, OpenDataChannelInput, CHANNEL_CLOSED,
    OUTPUT_STREAM_DATA, PAYLOAD_HANDSHAKE_COMPLETE, PAYLOAD_HANDSHAKE_REQUEST,
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

/// Type alias for the WebSocket stream used throughout.
pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Established SSM data channel ready for port forwarding.
pub struct SsmDataChannel {
    pub ws: WsStream,
    pub agent_version: String,
    /// Next outgoing sequence number (after handshake consumed some).
    pub outgoing_seq: i64,
    /// Next expected incoming sequence number (after handshake consumed some).
    pub expected_incoming_seq: i64,
}

/// Connect to SSM WebSocket, authenticate, and complete the handshake.
/// Returns a ready-to-use data channel.
///
/// Uses the basic client version ("1.0.0.0") which disables smux multiplexing.
pub async fn open_data_channel(
    stream_url: &str,
    token_value: &str,
) -> Result<SsmDataChannel, String> {
    open_data_channel_with_version(stream_url, token_value, crate::tunnel::smux::BASIC_CLIENT_VERSION).await
}

/// Connect to SSM WebSocket, authenticate, and complete the handshake
/// with a specific client version.
///
/// When `client_version` >= "1.1.70.0" and the agent supports it, the SSM agent
/// enables smux multiplexing mode where all data payloads are smux-framed.
pub async fn open_data_channel_with_version(
    stream_url: &str,
    token_value: &str,
    client_version: &str,
) -> Result<SsmDataChannel, String> {
    // Step 1: Connect WebSocket
    let (mut ws, _response) = connect_async(stream_url)
        .await
        .map_err(|e| format!("WebSocket connection failed: {}", e))?;

    // Step 2: Send OpenDataChannelInput as text message
    let open_msg = OpenDataChannelInput {
        message_schema_version: "1.0".to_string(),
        request_id: Uuid::new_v4().to_string(),
        token_value: token_value.to_string(),
        client_id: Uuid::new_v4().to_string(),
        client_version: client_version.to_string(),
    };
    let open_json = serde_json::to_string(&open_msg)
        .map_err(|e| format!("Failed to serialize OpenDataChannelInput: {}", e))?;
    ws.send(Message::Text(open_json.into()))
        .await
        .map_err(|e| format!("Failed to send OpenDataChannelInput: {}", e))?;

    // Step 3: Handshake — wait for HandshakeRequest from agent
    // Track sequence numbers: the agent uses a single counter for all outgoing messages,
    // and the client uses a single counter for all outgoing messages. These continue
    // from the handshake into data forwarding (they are NOT reset).
    let mut agent_version = String::new();
    let mut handshake_complete = false;
    let mut client_seq: i64 = 0; // our outgoing sequence counter
    let mut expected_server_seq: i64 = 0; // what we expect from the agent next

    while !handshake_complete {
        let ws_msg = ws
            .next()
            .await
            .ok_or_else(|| "WebSocket closed during handshake".to_string())?
            .map_err(|e| format!("WebSocket error during handshake: {}", e))?;

        match ws_msg {
            Message::Binary(data) => {
                let msg = AgentMessage::deserialize(&data)
                    .map_err(|e| format!("Failed to deserialize handshake message: {}", e))?;

                match msg.message_type.as_str() {
                    OUTPUT_STREAM_DATA => {
                        // Send acknowledge for every output_stream_data
                        let ack = build_acknowledge(&msg);
                        ws.send(Message::Binary(ack.serialize().into()))
                            .await
                            .map_err(|e| format!("Failed to send ack: {}", e))?;

                        // Advance expected server sequence (agent uses single counter)
                        expected_server_seq += 1;

                        match msg.payload_type {
                            PAYLOAD_HANDSHAKE_REQUEST => {
                                let request: HandshakeRequestPayload =
                                    serde_json::from_slice(&msg.payload).map_err(|e| {
                                        format!("Failed to parse HandshakeRequest: {}", e)
                                    })?;
                                agent_version = request.agent_version.clone();

                                // Send HandshakeResponse with the requested client version
                                let response = build_handshake_response_with_version(
                                    &request,
                                    client_seq,
                                    client_version,
                                );
                                ws.send(Message::Binary(response.serialize().into()))
                                    .await
                                    .map_err(|e| {
                                        format!("Failed to send HandshakeResponse: {}", e)
                                    })?;
                                client_seq += 1;
                            }
                            PAYLOAD_HANDSHAKE_COMPLETE => {
                                if let Ok(complete) =
                                    serde_json::from_slice::<HandshakeCompletePayload>(
                                        &msg.payload,
                                    )
                                {
                                    log::info!(
                                        "Handshake complete: {}",
                                        complete.customer_message
                                    );
                                }
                                handshake_complete = true;
                            }
                            _ => {
                                log::warn!(
                                    "Unexpected payload type {} during handshake",
                                    msg.payload_type
                                );
                            }
                        }
                    }
                    CHANNEL_CLOSED => {
                        return Err("Channel closed during handshake".to_string());
                    }
                    _ => {}
                }
            }
            Message::Close(_) => {
                return Err("WebSocket closed during handshake".to_string());
            }
            _ => {}
        }
    }

    log::info!(
        "Post-handshake seq: outgoing={}, expected_incoming={}",
        client_seq, expected_server_seq
    );

    Ok(SsmDataChannel {
        ws,
        agent_version,
        outgoing_seq: client_seq,
        expected_incoming_seq: expected_server_seq,
    })
}
