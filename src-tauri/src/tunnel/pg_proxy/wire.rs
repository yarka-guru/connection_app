//! Minimal PostgreSQL wire-protocol framing for the IAM auth proxy.
//!
//! Covers only the startup and authentication phases. Once the server sends
//! AuthenticationOk the proxy stops parsing and relays bytes untouched.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const SSL_REQUEST_CODE: u32 = 80877103;
pub const GSSENC_REQUEST_CODE: u32 = 80877104;
pub const CANCEL_REQUEST_CODE: u32 = 80877102;

/// PostgreSQL's own cap on a startup packet (MAX_STARTUP_PACKET_LENGTH).
pub const MAX_STARTUP_PACKET_LEN: usize = 10_000;
/// Cap on any regular message read before authentication completes.
pub const MAX_AUTH_MESSAGE_LEN: usize = 64 * 1024;

pub const AUTH_OK: u32 = 0;
pub const AUTH_CLEARTEXT_PASSWORD: u32 = 3;
pub const AUTH_MD5_PASSWORD: u32 = 5;
pub const AUTH_SASL: u32 = 10;
pub const AUTH_SASL_CONTINUE: u32 = 11;
pub const AUTH_SASL_FINAL: u32 = 12;

/// First packet a client sends on a fresh connection.
#[derive(Debug, PartialEq)]
pub enum StartupPacket {
    SslRequest,
    GssEncRequest,
    /// The whole packet, length prefix included, to forward verbatim.
    CancelRequest(Vec<u8>),
    Startup(StartupMessage),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartupMessage {
    pub protocol: u32,
    /// Parameters in the order the client sent them. Kept as raw bytes so
    /// they are forwarded exactly as received.
    pub params: Vec<(Vec<u8>, Vec<u8>)>,
}

impl StartupMessage {
    pub fn param(&self, name: &str) -> Option<&[u8]> {
        self.params
            .iter()
            .find(|(k, _)| k == name.as_bytes())
            .map(|(_, v)| v.as_slice())
    }

    /// Same message with `user` replaced (or added, first in order).
    pub fn with_user(&self, user: &str) -> StartupMessage {
        let mut params: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(self.params.len() + 1);
        params.push((b"user".to_vec(), user.as_bytes().to_vec()));
        params.extend(self.params.iter().filter(|(k, _)| k != b"user").cloned());
        StartupMessage {
            protocol: self.protocol,
            params,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&self.protocol.to_be_bytes());
        for (k, v) in &self.params {
            body.extend_from_slice(k);
            body.push(0);
            body.extend_from_slice(v);
            body.push(0);
        }
        body.push(0);
        with_length_prefix(body)
    }
}

/// A regular (tagged) protocol message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub tag: u8,
    pub body: Vec<u8>,
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        encode_message(self.tag, &self.body)
    }

    /// Authentication request subtype of an 'R' message.
    pub fn auth_code(&self) -> Option<u32> {
        if self.tag != b'R' || self.body.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([
            self.body[0],
            self.body[1],
            self.body[2],
            self.body[3],
        ]))
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn with_length_prefix(body: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn encode_message(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(tag);
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Read the untagged first packet of a connection.
pub async fn read_startup_packet<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<StartupPacket> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if !(8..=MAX_STARTUP_PACKET_LEN).contains(&len) {
        return Err(invalid("invalid length of startup packet"));
    }
    let mut body = vec![0u8; len - 4];
    r.read_exact(&mut body).await?;
    parse_startup_body(&body)
}

/// Parse a startup packet body (everything after the length prefix).
pub fn parse_startup_body(body: &[u8]) -> io::Result<StartupPacket> {
    if body.len() < 4 {
        return Err(invalid("invalid length of startup packet"));
    }
    let code = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    match code {
        SSL_REQUEST_CODE | GSSENC_REQUEST_CODE => {
            if body.len() != 4 {
                return Err(invalid("invalid length of encryption request"));
            }
            Ok(if code == SSL_REQUEST_CODE {
                StartupPacket::SslRequest
            } else {
                StartupPacket::GssEncRequest
            })
        }
        CANCEL_REQUEST_CODE => {
            // Process ID + secret key; the key is variable length in protocol 3.2.
            if body.len() < 12 {
                return Err(invalid("invalid length of cancel request"));
            }
            let mut packet = Vec::with_capacity(body.len() + 4);
            packet.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
            packet.extend_from_slice(body);
            Ok(StartupPacket::CancelRequest(packet))
        }
        _ if code >> 16 == 3 => Ok(StartupPacket::Startup(StartupMessage {
            protocol: code,
            params: parse_params(&body[4..])?,
        })),
        _ => Err(invalid("unsupported frontend protocol")),
    }
}

fn parse_params(mut data: &[u8]) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    fn cstr<'a>(data: &mut &'a [u8]) -> io::Result<&'a [u8]> {
        let end = data
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("unterminated string in startup packet"))?;
        let s = &data[..end];
        *data = &data[end + 1..];
        Ok(s)
    }

    let mut params = Vec::new();
    loop {
        let name = cstr(&mut data)?;
        if name.is_empty() {
            if !data.is_empty() {
                return Err(invalid("trailing data after startup packet terminator"));
            }
            return Ok(params);
        }
        let value = cstr(&mut data)?;
        params.push((name.to_vec(), value.to_vec()));
    }
}

/// Read one tagged message, refusing anything longer than `max_len`.
pub async fn read_message<R: AsyncRead + Unpin>(r: &mut R, max_len: usize) -> io::Result<Message> {
    let mut header = [0u8; 5];
    r.read_exact(&mut header).await?;
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len < 4 || len - 4 > max_len {
        return Err(invalid("invalid message length"));
    }
    let mut body = vec![0u8; len - 4];
    r.read_exact(&mut body).await?;
    Ok(Message {
        tag: header[0],
        body,
    })
}

pub fn ssl_request() -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&8u32.to_be_bytes());
    out[4..].copy_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
    out
}

pub fn auth_request(code: u32, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(data.len() + 4);
    body.extend_from_slice(&code.to_be_bytes());
    body.extend_from_slice(data);
    encode_message(b'R', &body)
}

pub fn password_message(password: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(password.len() + 1);
    body.extend_from_slice(password.as_bytes());
    body.push(0);
    encode_message(b'p', &body)
}

/// FATAL ErrorResponse as a PostgreSQL server would send it.
pub fn error_response(sqlstate: &str, message: &str, hint: Option<&str>) -> Vec<u8> {
    let mut body = Vec::new();
    for (field, value) in [
        (b'S', "FATAL"),
        (b'V', "FATAL"),
        (b'C', sqlstate),
        (b'M', message),
    ] {
        body.push(field);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    if let Some(hint) = hint {
        body.push(b'H');
        body.extend_from_slice(hint.as_bytes());
        body.push(0);
    }
    body.push(0);
    encode_message(b'E', &body)
}

/// One field of an ErrorResponse/NoticeResponse body (e.g. b'C' for SQLSTATE).
pub fn error_field(body: &[u8], field: u8) -> Option<String> {
    let mut rest = body;
    while let Some((&code, tail)) = rest.split_first() {
        if code == 0 {
            break;
        }
        let end = tail.iter().position(|&b| b == 0)?;
        if code == field {
            return Some(String::from_utf8_lossy(&tail[..end]).into_owned());
        }
        rest = &tail[end + 1..];
    }
    None
}

/// Mechanism names in an AuthenticationSASL body (after the auth code).
pub fn sasl_mechanisms(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.split(|&b| b == 0).take_while(|m| !m.is_empty())
}

/// SASLInitialResponse message for `mechanism` with the client-first-message.
pub fn sasl_initial_response(mechanism: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(mechanism.len() + 5 + data.len());
    body.extend_from_slice(mechanism.as_bytes());
    body.push(0);
    body.extend_from_slice(&(data.len() as i32).to_be_bytes());
    body.extend_from_slice(data);
    encode_message(b'p', &body)
}

/// SASLInitialResponse body: mechanism name and the client-first-message.
pub fn parse_sasl_initial_response(body: &[u8]) -> io::Result<(&[u8], &[u8])> {
    let end = body
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| invalid("malformed SASLInitialResponse"))?;
    let mechanism = &body[..end];
    let rest = &body[end + 1..];
    if rest.len() < 4 {
        return Err(invalid("malformed SASLInitialResponse"));
    }
    let len = i32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
    let data = &rest[4..];
    if len < 0 || len as usize != data.len() {
        return Err(invalid("malformed SASLInitialResponse"));
    }
    Ok((mechanism, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    /// Yields at most one byte per read, so every parser path sees the
    /// smallest possible partial reads.
    struct OneByteReader {
        data: Vec<u8>,
        pos: usize,
    }

    impl OneByteReader {
        fn new(data: Vec<u8>) -> Self {
            Self { data, pos: 0 }
        }
    }

    impl AsyncRead for OneByteReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.pos < self.data.len() && buf.remaining() > 0 {
                let b = self.data[self.pos];
                buf.put_slice(&[b]);
                self.pos += 1;
            }
            Poll::Ready(Ok(()))
        }
    }

    fn startup_bytes(protocol: u32, params: &[(&str, &str)]) -> Vec<u8> {
        StartupMessage {
            protocol,
            params: params
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect(),
        }
        .encode()
    }

    #[tokio::test]
    async fn parses_startup_message_across_one_byte_reads() {
        let bytes = startup_bytes(
            196608,
            &[("user", "covered_app"), ("database", "covered_db"), ("application_name", "psql")],
        );
        let mut r = OneByteReader::new(bytes);
        let StartupPacket::Startup(msg) = read_startup_packet(&mut r).await.unwrap() else {
            panic!("expected startup message");
        };
        assert_eq!(msg.protocol, 196608);
        assert_eq!(msg.param("user"), Some(&b"covered_app"[..]));
        assert_eq!(msg.param("database"), Some(&b"covered_db"[..]));
        assert_eq!(msg.param("application_name"), Some(&b"psql"[..]));
        assert_eq!(msg.param("options"), None);
    }

    #[tokio::test]
    async fn recognises_ssl_gssenc_and_cancel_codes() {
        let mut r = OneByteReader::new(ssl_request().to_vec());
        assert_eq!(read_startup_packet(&mut r).await.unwrap(), StartupPacket::SslRequest);

        let mut gss = 8u32.to_be_bytes().to_vec();
        gss.extend_from_slice(&GSSENC_REQUEST_CODE.to_be_bytes());
        let mut r = OneByteReader::new(gss);
        assert_eq!(read_startup_packet(&mut r).await.unwrap(), StartupPacket::GssEncRequest);

        let mut cancel = 16u32.to_be_bytes().to_vec();
        cancel.extend_from_slice(&CANCEL_REQUEST_CODE.to_be_bytes());
        cancel.extend_from_slice(&4242u32.to_be_bytes());
        cancel.extend_from_slice(&0xdead_beefu32.to_be_bytes());
        let mut r = OneByteReader::new(cancel.clone());
        assert_eq!(
            read_startup_packet(&mut r).await.unwrap(),
            StartupPacket::CancelRequest(cancel),
            "cancel packet must be forwarded byte for byte"
        );
    }

    #[tokio::test]
    async fn cancel_request_with_protocol_3_2_long_key_is_kept_whole() {
        let mut cancel = (4u32 + 4 + 4 + 32).to_be_bytes().to_vec();
        cancel.extend_from_slice(&CANCEL_REQUEST_CODE.to_be_bytes());
        cancel.extend_from_slice(&7u32.to_be_bytes());
        cancel.extend_from_slice(&[0xab; 32]);
        let mut r = OneByteReader::new(cancel.clone());
        assert_eq!(
            read_startup_packet(&mut r).await.unwrap(),
            StartupPacket::CancelRequest(cancel)
        );
    }

    #[tokio::test]
    async fn rejects_bad_startup_packets() {
        // Length below the minimum
        let mut r = OneByteReader::new(4u32.to_be_bytes().to_vec());
        assert!(read_startup_packet(&mut r).await.is_err());

        // Length above PostgreSQL's limit
        let mut r = OneByteReader::new(((MAX_STARTUP_PACKET_LEN + 1) as u32).to_be_bytes().to_vec());
        assert!(read_startup_packet(&mut r).await.is_err());

        // Protocol 2
        let mut r = OneByteReader::new(startup_bytes(0x0002_0000, &[("user", "x")]));
        assert!(read_startup_packet(&mut r).await.is_err());

        // Missing the final terminator
        let mut bad = startup_bytes(196608, &[("user", "x")]);
        bad.pop();
        let len = (bad.len() as u32).to_be_bytes();
        bad[..4].copy_from_slice(&len);
        let mut r = OneByteReader::new(bad);
        assert!(read_startup_packet(&mut r).await.is_err());

        // SSLRequest with a trailing payload
        let mut ssl = 12u32.to_be_bytes().to_vec();
        ssl.extend_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
        ssl.extend_from_slice(&[0, 0, 0, 0]);
        let mut r = OneByteReader::new(ssl);
        assert!(read_startup_packet(&mut r).await.is_err());

        // Truncated stream
        let mut truncated = startup_bytes(196608, &[("user", "x")]);
        truncated.truncate(10);
        let mut r = OneByteReader::new(truncated);
        assert!(read_startup_packet(&mut r).await.is_err());
    }

    #[test]
    fn with_user_replaces_user_and_keeps_the_rest_in_order() {
        let msg = StartupMessage {
            protocol: 196610,
            params: vec![
                (b"database".to_vec(), b"covered_db".to_vec()),
                (b"user".to_vec(), b"covered_app".to_vec()),
                (b"options".to_vec(), b"-c search_path=x".to_vec()),
            ],
        };
        let mapped = msg.with_user("iam_user");
        assert_eq!(mapped.protocol, 196610);
        assert_eq!(
            mapped.params,
            vec![
                (b"user".to_vec(), b"iam_user".to_vec()),
                (b"database".to_vec(), b"covered_db".to_vec()),
                (b"options".to_vec(), b"-c search_path=x".to_vec()),
            ]
        );
        let StartupPacket::Startup(round_trip) = parse_startup_body(&mapped.encode()[4..]).unwrap() else {
            panic!("expected startup message");
        };
        assert_eq!(round_trip, mapped);
    }

    #[tokio::test]
    async fn reads_tagged_messages_across_one_byte_reads() {
        let mut stream = auth_request(AUTH_SASL_CONTINUE, b"r=abc,s=c2FsdA==,i=4096");
        stream.extend_from_slice(&auth_request(AUTH_OK, &[]));
        let mut r = OneByteReader::new(stream);

        let first = read_message(&mut r, MAX_AUTH_MESSAGE_LEN).await.unwrap();
        assert_eq!(first.tag, b'R');
        assert_eq!(first.auth_code(), Some(AUTH_SASL_CONTINUE));
        assert_eq!(&first.body[4..], b"r=abc,s=c2FsdA==,i=4096");

        let second = read_message(&mut r, MAX_AUTH_MESSAGE_LEN).await.unwrap();
        assert_eq!(second.auth_code(), Some(AUTH_OK));
        assert_eq!(second.encode(), auth_request(AUTH_OK, &[]));
    }

    #[tokio::test]
    async fn read_message_enforces_the_length_cap() {
        let big = encode_message(b'p', &[b'x'; 100]);
        let mut r = OneByteReader::new(big.clone());
        assert!(read_message(&mut r, 99).await.is_err());
        let mut r = OneByteReader::new(big);
        assert!(read_message(&mut r, 100).await.is_ok());

        let mut bogus = vec![b'p'];
        bogus.extend_from_slice(&3u32.to_be_bytes());
        let mut r = OneByteReader::new(bogus);
        assert!(read_message(&mut r, 100).await.is_err());
    }

    #[test]
    fn error_response_round_trips_fields() {
        let msg = error_response("28P01", "password authentication failed for user \"x\"", Some("hint"));
        assert_eq!(msg[0], b'E');
        let body = &msg[5..];
        assert_eq!(error_field(body, b'S').as_deref(), Some("FATAL"));
        assert_eq!(error_field(body, b'C').as_deref(), Some("28P01"));
        assert_eq!(
            error_field(body, b'M').as_deref(),
            Some("password authentication failed for user \"x\"")
        );
        assert_eq!(error_field(body, b'H').as_deref(), Some("hint"));
        assert_eq!(error_field(body, b'D'), None);
    }

    #[test]
    fn lists_sasl_mechanisms() {
        let offered: Vec<&[u8]> = sasl_mechanisms(b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0").collect();
        assert_eq!(offered, vec![&b"SCRAM-SHA-256-PLUS"[..], &b"SCRAM-SHA-256"[..]]);
        assert_eq!(sasl_mechanisms(b"\0").count(), 0);
    }

    #[test]
    fn parses_sasl_initial_response() {
        let msg = sasl_initial_response("SCRAM-SHA-256", b"n,,n=");
        assert_eq!(msg[0], b'p');
        let body = msg[5..].to_vec();
        let (mechanism, data) = parse_sasl_initial_response(&body).unwrap();
        assert_eq!(mechanism, b"SCRAM-SHA-256");
        assert_eq!(data, b"n,,n=");

        let mut short = b"SCRAM-SHA-256\0".to_vec();
        short.extend_from_slice(&9i32.to_be_bytes());
        short.extend_from_slice(b"n,,");
        assert!(parse_sasl_initial_response(&short).is_err());
    }
}
