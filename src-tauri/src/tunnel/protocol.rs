use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use uuid::Uuid;

/// Header length value written to the wire (does NOT include PayloadLength's 4 bytes).
const HEADER_LENGTH: u32 = 116;
/// Total bytes before payload starts (header + PayloadLength field).
const TOTAL_HEADER_SIZE: usize = 120;
/// Size of the MessageType field in bytes.
const MESSAGE_TYPE_SIZE: usize = 32;
/// Size of the PayloadDigest field (SHA-256) in bytes.
const PAYLOAD_DIGEST_SIZE: usize = 32;
/// Schema version used by the protocol.
const SCHEMA_VERSION: u32 = 1;

// --- Field offsets ---
const HL_OFFSET: usize = 0;
const MESSAGE_TYPE_OFFSET: usize = 4;
const SCHEMA_VERSION_OFFSET: usize = 36;
const CREATED_DATE_OFFSET: usize = 40;
const SEQUENCE_NUMBER_OFFSET: usize = 48;
const FLAGS_OFFSET: usize = 56;
const MESSAGE_ID_OFFSET: usize = 64;
const PAYLOAD_DIGEST_OFFSET: usize = 80;
const PAYLOAD_TYPE_OFFSET: usize = 112;
const PAYLOAD_LENGTH_OFFSET: usize = 116;

// --- Message types ---
pub const INPUT_STREAM_DATA: &str = "input_stream_data";
pub const OUTPUT_STREAM_DATA: &str = "output_stream_data";
pub const ACKNOWLEDGE: &str = "acknowledge";
pub const CHANNEL_CLOSED: &str = "channel_closed";
#[allow(dead_code)]
pub const PAUSE_PUBLICATION: &str = "pause_publication";
#[allow(dead_code)]
pub const START_PUBLICATION: &str = "start_publication";

// --- Payload types ---
pub const PAYLOAD_OUTPUT: u32 = 1;
#[allow(dead_code)]
pub const PAYLOAD_ERROR: u32 = 2;
pub const PAYLOAD_HANDSHAKE_REQUEST: u32 = 5;
pub const PAYLOAD_HANDSHAKE_RESPONSE: u32 = 6;
pub const PAYLOAD_HANDSHAKE_COMPLETE: u32 = 7;
pub const PAYLOAD_FLAG: u32 = 10;

// --- Flag values ---
pub const FLAG_DATA: u64 = 0;
pub const FLAG_SYN: u64 = 1;
pub const FLAG_FIN: u64 = 2;
pub const FLAG_ACK: u64 = 3;

// --- PayloadType Flag values (payload content when PayloadType = FLAG) ---
pub const FLAG_DISCONNECT_TO_PORT: u32 = 1;
pub const FLAG_TERMINATE_SESSION: u32 = 2;
#[allow(dead_code)]
pub const FLAG_CONNECT_TO_PORT_ERROR: u32 = 3;

/// Max bytes to read from local TCP per message (basic mode).
pub const STREAM_DATA_PAYLOAD_SIZE: usize = 1024;

/// A decoded SSM agent/client message.
#[derive(Debug, Clone)]
pub struct AgentMessage {
    pub header_length: u32,
    pub message_type: String,
    pub schema_version: u32,
    pub created_date: u64,
    pub sequence_number: i64,
    pub flags: u64,
    pub message_id: Uuid,
    pub payload_type: u32,
    pub payload: Vec<u8>,
}

impl AgentMessage {
    /// Create a new message with current timestamp and random UUID.
    pub fn new(message_type: &str, sequence_number: i64, flags: u64, payload_type: u32, payload: Vec<u8>) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            header_length: HEADER_LENGTH,
            message_type: message_type.to_string(),
            schema_version: SCHEMA_VERSION,
            created_date: now_ms,
            sequence_number,
            flags,
            message_id: Uuid::new_v4(),
            payload_type,
            payload,
        }
    }

    /// Serialize this message to bytes for sending over WebSocket.
    /// All writes target pre-sized buffer slices and cannot fail at runtime.
    pub fn serialize(&self) -> Vec<u8> {
        let total_len = TOTAL_HEADER_SIZE + self.payload.len();
        let mut buf = vec![0u8; total_len];

        // HeaderLength (u32 at offset 0)
        let mut cursor = Cursor::new(&mut buf[HL_OFFSET..HL_OFFSET + 4]);
        cursor.write_u32::<BigEndian>(self.header_length).expect("write u32 to 4-byte slice");

        // MessageType (32 bytes, space-padded, at offset 4)
        let mt_bytes = &mut buf[MESSAGE_TYPE_OFFSET..MESSAGE_TYPE_OFFSET + MESSAGE_TYPE_SIZE];
        mt_bytes.fill(b' ');
        let src = self.message_type.as_bytes();
        let copy_len = src.len().min(MESSAGE_TYPE_SIZE);
        mt_bytes[..copy_len].copy_from_slice(&src[..copy_len]);

        // SchemaVersion (u32 at offset 36)
        let mut cursor = Cursor::new(&mut buf[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4]);
        cursor.write_u32::<BigEndian>(self.schema_version).expect("write u32 to 4-byte slice");

        // CreatedDate (u64 at offset 40)
        let mut cursor = Cursor::new(&mut buf[CREATED_DATE_OFFSET..CREATED_DATE_OFFSET + 8]);
        cursor.write_u64::<BigEndian>(self.created_date).expect("write u64 to 8-byte slice");

        // SequenceNumber (i64 at offset 48)
        let mut cursor = Cursor::new(&mut buf[SEQUENCE_NUMBER_OFFSET..SEQUENCE_NUMBER_OFFSET + 8]);
        cursor.write_i64::<BigEndian>(self.sequence_number).expect("write i64 to 8-byte slice");

        // Flags (u64 at offset 56)
        let mut cursor = Cursor::new(&mut buf[FLAGS_OFFSET..FLAGS_OFFSET + 8]);
        cursor.write_u64::<BigEndian>(self.flags).expect("write u64 to 8-byte slice");

        // MessageId (UUID, 16 bytes at offset 64, byte-swapped halves)
        let uuid_bytes = self.message_id.as_bytes();
        // Wire layout: LSB half (bytes[8..16]) first, MSB half (bytes[0..8]) second
        buf[MESSAGE_ID_OFFSET..MESSAGE_ID_OFFSET + 8].copy_from_slice(&uuid_bytes[8..16]);
        buf[MESSAGE_ID_OFFSET + 8..MESSAGE_ID_OFFSET + 16].copy_from_slice(&uuid_bytes[0..8]);

        // Payload (at offset 120+)
        if !self.payload.is_empty() {
            buf[TOTAL_HEADER_SIZE..].copy_from_slice(&self.payload);
        }

        // PayloadDigest (SHA-256 of payload, 32 bytes at offset 80)
        let digest = Sha256::digest(&self.payload);
        buf[PAYLOAD_DIGEST_OFFSET..PAYLOAD_DIGEST_OFFSET + PAYLOAD_DIGEST_SIZE]
            .copy_from_slice(&digest);

        // PayloadType (u32 at offset 112)
        let mut cursor = Cursor::new(&mut buf[PAYLOAD_TYPE_OFFSET..PAYLOAD_TYPE_OFFSET + 4]);
        cursor.write_u32::<BigEndian>(self.payload_type).expect("write u32 to 4-byte slice");

        // PayloadLength (u32 at offset 116)
        let mut cursor = Cursor::new(&mut buf[PAYLOAD_LENGTH_OFFSET..PAYLOAD_LENGTH_OFFSET + 4]);
        cursor.write_u32::<BigEndian>(self.payload.len() as u32).expect("write u32 to 4-byte slice");

        buf
    }

    /// Deserialize a message from raw bytes received over WebSocket.
    pub fn deserialize(input: &[u8]) -> Result<Self, String> {
        if input.len() < TOTAL_HEADER_SIZE {
            return Err(format!(
                "Message too short: {} bytes, need at least {}",
                input.len(),
                TOTAL_HEADER_SIZE
            ));
        }

        // HeaderLength
        let header_length = Cursor::new(&input[HL_OFFSET..HL_OFFSET + 4])
            .read_u32::<BigEndian>()
            .map_err(|e| format!("Failed to read HeaderLength: {}", e))?;

        // MessageType (trim nulls then spaces)
        let mt_raw = &input[MESSAGE_TYPE_OFFSET..MESSAGE_TYPE_OFFSET + MESSAGE_TYPE_SIZE];
        let message_type = String::from_utf8_lossy(mt_raw)
            .trim_end_matches('\0')
            .trim()
            .to_string();

        // SchemaVersion
        let schema_version = Cursor::new(&input[SCHEMA_VERSION_OFFSET..SCHEMA_VERSION_OFFSET + 4])
            .read_u32::<BigEndian>()
            .map_err(|e| format!("Failed to read SchemaVersion: {}", e))?;

        // CreatedDate
        let created_date = Cursor::new(&input[CREATED_DATE_OFFSET..CREATED_DATE_OFFSET + 8])
            .read_u64::<BigEndian>()
            .map_err(|e| format!("Failed to read CreatedDate: {}", e))?;

        // SequenceNumber
        let sequence_number =
            Cursor::new(&input[SEQUENCE_NUMBER_OFFSET..SEQUENCE_NUMBER_OFFSET + 8])
                .read_i64::<BigEndian>()
                .map_err(|e| format!("Failed to read SequenceNumber: {}", e))?;

        // Flags
        let flags = Cursor::new(&input[FLAGS_OFFSET..FLAGS_OFFSET + 8])
            .read_u64::<BigEndian>()
            .map_err(|e| format!("Failed to read Flags: {}", e))?;

        // MessageId (UUID, byte-swapped halves)
        let mut uuid_bytes = [0u8; 16];
        // Wire: LSB at offset, MSB at offset+8 → standard: MSB first, LSB second
        uuid_bytes[0..8].copy_from_slice(&input[MESSAGE_ID_OFFSET + 8..MESSAGE_ID_OFFSET + 16]);
        uuid_bytes[8..16].copy_from_slice(&input[MESSAGE_ID_OFFSET..MESSAGE_ID_OFFSET + 8]);
        let message_id = Uuid::from_bytes(uuid_bytes);

        // PayloadType (may be absent for channel_closed with HeaderLength=112)
        let payload_type = if header_length >= 116 {
            Cursor::new(&input[PAYLOAD_TYPE_OFFSET..PAYLOAD_TYPE_OFFSET + 4])
                .read_u32::<BigEndian>()
                .map_err(|e| format!("Failed to read PayloadType: {}", e))?
        } else {
            0
        };

        // PayloadLength (at header_length offset)
        let pl_offset = header_length as usize;
        if input.len() < pl_offset + 4 {
            return Err(format!(
                "Message too short for PayloadLength at offset {}",
                pl_offset
            ));
        }
        let payload_length = Cursor::new(&input[pl_offset..pl_offset + 4])
            .read_u32::<BigEndian>()
            .map_err(|e| format!("Failed to read PayloadLength: {}", e))?;

        // Payload
        let payload_start = pl_offset + 4;
        let payload_end = payload_start + payload_length as usize;
        if input.len() < payload_end {
            return Err(format!(
                "Message truncated: expected {} payload bytes, have {}",
                payload_length,
                input.len() - payload_start
            ));
        }
        let payload = input[payload_start..payload_end].to_vec();

        Ok(Self {
            header_length,
            message_type,
            schema_version,
            created_date,
            sequence_number,
            flags,
            message_id,
            payload_type,
            payload,
        })
    }

    /// Validate the payload digest (SHA-256).
    pub fn validate_digest(&self, raw: &[u8]) -> bool {
        if raw.len() < PAYLOAD_DIGEST_OFFSET + PAYLOAD_DIGEST_SIZE {
            return false;
        }
        let stored = &raw[PAYLOAD_DIGEST_OFFSET..PAYLOAD_DIGEST_OFFSET + PAYLOAD_DIGEST_SIZE];
        let computed = Sha256::digest(&self.payload);
        stored == computed.as_slice()
    }
}

/// JSON payload for the acknowledge message.
#[derive(serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AcknowledgeContent {
    pub acknowledged_message_type: String,
    pub acknowledged_message_id: String,
    pub acknowledged_message_sequence_number: i64,
    pub is_sequential_message: bool,
}

/// Build an acknowledge message for a received message.
pub fn build_acknowledge(original: &AgentMessage) -> AgentMessage {
    let ack_content = AcknowledgeContent {
        acknowledged_message_type: original.message_type.clone(),
        acknowledged_message_id: original.message_id.to_string(),
        acknowledged_message_sequence_number: original.sequence_number,
        is_sequential_message: true,
    };
    let payload = serde_json::to_vec(&ack_content).unwrap_or_default();

    AgentMessage::new(ACKNOWLEDGE, 0, FLAG_ACK, 0, payload)
}

/// Build an input_stream_data message carrying TCP data.
/// The Go session-manager-plugin always sends Flags=0 for outgoing data messages.
pub fn build_data_message(data: &[u8], sequence_number: i64) -> AgentMessage {
    AgentMessage::new(
        INPUT_STREAM_DATA,
        sequence_number,
        FLAG_DATA,
        PAYLOAD_OUTPUT,
        data.to_vec(),
    )
}

/// Build a SYN flag message — sent after handshake to tell the SSM agent
/// that the client is ready for port forwarding. The Go session-manager-plugin
/// sends this; without it the agent may not connect to the remote host.
pub fn build_syn_message(sequence_number: i64) -> AgentMessage {
    AgentMessage::new(INPUT_STREAM_DATA, sequence_number, FLAG_SYN, PAYLOAD_FLAG, vec![])
}

/// Build a flag message (DisconnectToPort, TerminateSession, etc.).
pub fn build_flag_message(flag_value: u32, sequence_number: i64, fin: bool) -> AgentMessage {
    let mut payload = vec![0u8; 4];
    let mut cursor = Cursor::new(&mut payload[..]);
    // Safe: 4-byte buffer is exactly sized for a u32
    cursor.write_u32::<BigEndian>(flag_value).expect("write u32 to 4-byte buffer");

    let flags = if fin { FLAG_FIN } else { FLAG_DATA };
    AgentMessage::new(INPUT_STREAM_DATA, sequence_number, flags, PAYLOAD_FLAG, payload)
}

// --- Handshake JSON structures ---

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct HandshakeRequestPayload {
    pub agent_version: String,
    pub requested_client_actions: Vec<RequestedClientAction>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct RequestedClientAction {
    pub action_type: String,
    #[allow(dead_code)]
    pub action_parameters: serde_json::Value,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HandshakeResponsePayload {
    pub client_version: String,
    pub processed_client_actions: Vec<ProcessedClientAction>,
    pub errors: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProcessedClientAction {
    pub action_type: String,
    pub action_status: u32,
    pub action_result: Option<serde_json::Value>,
    pub error: String,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct HandshakeCompletePayload {
    #[allow(dead_code)]
    pub handshake_time_to_complete: i64,
    pub customer_message: String,
}

/// Build the HandshakeResponse message.
///
/// `client_version` controls whether the SSM agent enables smux multiplexing:
/// - `"1.0.0.0"` (basic mode): single TCP connection per tunnel
/// - `"1.2.0.0"` (multiplexed mode): multiple TCP connections via smux framing
pub fn build_handshake_response(
    request: &HandshakeRequestPayload,
    sequence_number: i64,
) -> AgentMessage {
    build_handshake_response_with_version(request, sequence_number, "1.0.0.0")
}

/// Build the HandshakeResponse message with a specific client version.
pub fn build_handshake_response_with_version(
    request: &HandshakeRequestPayload,
    sequence_number: i64,
    client_version: &str,
) -> AgentMessage {
    let processed = request
        .requested_client_actions
        .iter()
        .map(|action| ProcessedClientAction {
            action_type: action.action_type.clone(),
            action_status: 1, // Success
            action_result: None,
            error: String::new(),
        })
        .collect();

    let response = HandshakeResponsePayload {
        client_version: client_version.to_string(),
        processed_client_actions: processed,
        errors: vec![],
    };

    let payload = serde_json::to_vec(&response).unwrap_or_default();
    AgentMessage::new(
        INPUT_STREAM_DATA,
        sequence_number,
        FLAG_DATA,
        PAYLOAD_HANDSHAKE_RESPONSE,
        payload,
    )
}

/// JSON message sent as WebSocket text to authenticate the data channel.
#[derive(serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct OpenDataChannelInput {
    pub message_schema_version: String,
    pub request_id: String,
    pub token_value: String,
    pub client_id: String,
    pub client_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_deserialize_roundtrip() {
        let payload = b"hello world".to_vec();
        let msg = AgentMessage::new(INPUT_STREAM_DATA, 42, FLAG_DATA, PAYLOAD_OUTPUT, payload.clone());

        let bytes = msg.serialize();
        assert_eq!(bytes.len(), TOTAL_HEADER_SIZE + payload.len());

        let decoded = AgentMessage::deserialize(&bytes).unwrap();
        assert_eq!(decoded.header_length, HEADER_LENGTH);
        assert_eq!(decoded.message_type, INPUT_STREAM_DATA);
        assert_eq!(decoded.schema_version, SCHEMA_VERSION);
        assert_eq!(decoded.sequence_number, 42);
        assert_eq!(decoded.flags, FLAG_DATA);
        assert_eq!(decoded.message_id, msg.message_id);
        assert_eq!(decoded.payload_type, PAYLOAD_OUTPUT);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn uuid_byte_swap_roundtrip() {
        let original_uuid = Uuid::new_v4();
        let msg = AgentMessage {
            header_length: HEADER_LENGTH,
            message_type: "test".to_string(),
            schema_version: SCHEMA_VERSION,
            created_date: 0,
            sequence_number: 0,
            flags: 0,
            message_id: original_uuid,
            payload_type: 0,
            payload: vec![],
        };

        let bytes = msg.serialize();
        let decoded = AgentMessage::deserialize(&bytes).unwrap();
        assert_eq!(decoded.message_id, original_uuid);
    }

    #[test]
    fn uuid_byte_swap_wire_format() {
        // Verify the on-wire layout matches the protocol spec:
        // wire[64..72] = uuid_bytes[8..16] (LSB half first)
        // wire[72..80] = uuid_bytes[0..8]  (MSB half second)
        let uuid = Uuid::from_bytes([
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, // MSB half (bytes 0..8)
            0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00, // LSB half (bytes 8..16)
        ]);
        let msg = AgentMessage {
            header_length: HEADER_LENGTH,
            message_type: "test".to_string(),
            schema_version: SCHEMA_VERSION,
            created_date: 0,
            sequence_number: 0,
            flags: 0,
            message_id: uuid,
            payload_type: 0,
            payload: vec![],
        };
        let bytes = msg.serialize();

        // On wire: LSB half at offset 64, MSB half at offset 72
        assert_eq!(
            &bytes[MESSAGE_ID_OFFSET..MESSAGE_ID_OFFSET + 8],
            &[0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00]
        );
        assert_eq!(
            &bytes[MESSAGE_ID_OFFSET + 8..MESSAGE_ID_OFFSET + 16],
            &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22]
        );
    }

    #[test]
    fn message_type_space_padded() {
        let msg = AgentMessage::new("ack", 0, 0, 0, vec![]);
        let bytes = msg.serialize();

        // "ack" is 3 chars, rest should be spaces
        let mt_field = &bytes[MESSAGE_TYPE_OFFSET..MESSAGE_TYPE_OFFSET + MESSAGE_TYPE_SIZE];
        assert_eq!(&mt_field[..3], b"ack");
        assert!(mt_field[3..].iter().all(|&b| b == b' '));

        // Roundtrip trims correctly
        let decoded = AgentMessage::deserialize(&bytes).unwrap();
        assert_eq!(decoded.message_type, "ack");
    }

    #[test]
    fn payload_digest_is_correct() {
        let payload = b"test payload data".to_vec();
        let msg = AgentMessage::new(INPUT_STREAM_DATA, 0, 0, PAYLOAD_OUTPUT, payload.clone());
        let bytes = msg.serialize();

        let expected_digest = Sha256::digest(&payload);
        let wire_digest =
            &bytes[PAYLOAD_DIGEST_OFFSET..PAYLOAD_DIGEST_OFFSET + PAYLOAD_DIGEST_SIZE];
        assert_eq!(wire_digest, expected_digest.as_slice());
    }

    #[test]
    fn empty_payload_message() {
        let msg = AgentMessage::new(ACKNOWLEDGE, 0, FLAG_ACK, 0, vec![]);
        let bytes = msg.serialize();
        assert_eq!(bytes.len(), TOTAL_HEADER_SIZE);

        let decoded = AgentMessage::deserialize(&bytes).unwrap();
        assert!(decoded.payload.is_empty());
        assert_eq!(decoded.flags, FLAG_ACK);
    }

    #[test]
    fn build_acknowledge_message() {
        let original = AgentMessage::new(OUTPUT_STREAM_DATA, 5, FLAG_DATA, PAYLOAD_OUTPUT, vec![1, 2, 3]);
        let ack = build_acknowledge(&original);

        assert_eq!(ack.message_type, ACKNOWLEDGE);
        assert_eq!(ack.flags, FLAG_ACK);
        assert_eq!(ack.sequence_number, 0);

        let content: serde_json::Value = serde_json::from_slice(&ack.payload).unwrap();
        assert_eq!(content["AcknowledgedMessageType"], OUTPUT_STREAM_DATA);
        assert_eq!(
            content["AcknowledgedMessageId"],
            original.message_id.to_string()
        );
        assert_eq!(content["AcknowledgedMessageSequenceNumber"], 5);
        assert_eq!(content["IsSequentialMessage"], true);
    }

    #[test]
    fn build_flag_message_terminate() {
        let msg = build_flag_message(FLAG_TERMINATE_SESSION, 10, true);
        assert_eq!(msg.message_type, INPUT_STREAM_DATA);
        assert_eq!(msg.payload_type, PAYLOAD_FLAG);
        assert_eq!(msg.flags, FLAG_FIN);
        assert_eq!(msg.sequence_number, 10);

        // Payload should be uint32 big-endian 2
        assert_eq!(msg.payload, vec![0, 0, 0, 2]);
    }

    #[test]
    fn handshake_response_serialization() {
        let request = HandshakeRequestPayload {
            agent_version: "3.1.1511.0".to_string(),
            requested_client_actions: vec![RequestedClientAction {
                action_type: "SessionType".to_string(),
                action_parameters: serde_json::json!({"SessionType": "Port"}),
            }],
        };

        let msg = build_handshake_response(&request, 0);
        assert_eq!(msg.message_type, INPUT_STREAM_DATA);
        assert_eq!(msg.payload_type, PAYLOAD_HANDSHAKE_RESPONSE);

        let response: HandshakeResponsePayload =
            serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(response.client_version, "1.0.0.0");
        assert_eq!(response.processed_client_actions.len(), 1);
        assert_eq!(response.processed_client_actions[0].action_status, 1);
    }

    #[test]
    fn deserialize_too_short() {
        let result = AgentMessage::deserialize(&[0u8; 50]);
        assert!(result.is_err());
    }
}
