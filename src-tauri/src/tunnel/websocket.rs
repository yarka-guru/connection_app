use crate::tunnel::protocol::{
    build_acknowledge, build_handshake_response, AgentMessage, HandshakeCompletePayload,
    HandshakeRequestPayload, OpenDataChannelInput, CHANNEL_CLOSED, OUTPUT_STREAM_DATA,
    PAYLOAD_HANDSHAKE_COMPLETE, PAYLOAD_HANDSHAKE_REQUEST,
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
}

/// Connect to SSM WebSocket, authenticate, and complete the handshake.
/// Returns a ready-to-use data channel.
pub async fn open_data_channel(
    stream_url: &str,
    token_value: &str,
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
        client_version: "1.0.0.0".to_string(),
    };
    let open_json = serde_json::to_string(&open_msg)
        .map_err(|e| format!("Failed to serialize OpenDataChannelInput: {}", e))?;
    ws.send(Message::Text(open_json.into()))
        .await
        .map_err(|e| format!("Failed to send OpenDataChannelInput: {}", e))?;

    // Step 3: Handshake — wait for HandshakeRequest from agent
    let mut agent_version = String::new();
    let mut handshake_complete = false;

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

                        match msg.payload_type {
                            PAYLOAD_HANDSHAKE_REQUEST => {
                                let request: HandshakeRequestPayload =
                                    serde_json::from_slice(&msg.payload).map_err(|e| {
                                        format!("Failed to parse HandshakeRequest: {}", e)
                                    })?;
                                agent_version = request.agent_version.clone();

                                // Send HandshakeResponse
                                let response = build_handshake_response(&request, 0);
                                ws.send(Message::Binary(response.serialize().into()))
                                    .await
                                    .map_err(|e| {
                                        format!("Failed to send HandshakeResponse: {}", e)
                                    })?;
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

    Ok(SsmDataChannel {
        ws,
        agent_version,
    })
}
