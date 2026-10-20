//! Server side of SCRAM-SHA-256 (RFC 5802 / RFC 7677) as PostgreSQL speaks it.
//!
//! Channel binding is not offered: the client hop is plaintext loopback.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const MECHANISM: &str = "SCRAM-SHA-256";
pub const DEFAULT_ITERATIONS: u32 = 4096;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, PartialEq)]
pub enum ScramError {
    Malformed(&'static str),
    InvalidProof,
}

/// What the server stores for a password: never the password itself.
pub struct ScramVerifier {
    salt: Vec<u8>,
    iterations: u32,
    stored_key: [u8; 32],
    server_key: [u8; 32],
}

/// Exchange state between server-first and client-final.
pub struct ScramExchange {
    gs2_header: String,
    client_first_bare: String,
    server_first: String,
    nonce: String,
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

/// Hi() from RFC 5802: PBKDF2 with HMAC-SHA-256 and a single output block.
fn hi(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut first = Vec::with_capacity(salt.len() + 4);
    first.extend_from_slice(salt);
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &first);
    let mut result = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (r, b) in result.iter_mut().zip(u) {
            *r ^= b;
        }
    }
    result
}

fn is_attribute(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b'='
}

fn is_printable_nonce(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7e).contains(&b) && b != b',')
}

impl ScramVerifier {
    pub fn new(password: &str, salt: Vec<u8>, iterations: u32) -> Self {
        let salted = hi(password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        Self {
            stored_key: sha256(&client_key),
            server_key: hmac(&salted, b"Server Key"),
            salt,
            iterations,
        }
    }

    /// Handle client-first-message; returns the state and server-first-message.
    pub fn server_first(
        &self,
        client_first: &[u8],
        server_nonce: &str,
    ) -> Result<(ScramExchange, String), ScramError> {
        let msg = std::str::from_utf8(client_first)
            .map_err(|_| ScramError::Malformed("client-first-message is not UTF-8"))?;

        let (gs2_header, bare) = match msg.as_bytes().first() {
            Some(b'n') | Some(b'y') => {
                let rest = msg
                    .get(1..)
                    .and_then(|r| r.strip_prefix(','))
                    .ok_or(ScramError::Malformed("malformed GS2 header"))?;
                let authzid_end = rest
                    .find(',')
                    .ok_or(ScramError::Malformed("malformed GS2 header"))?;
                if authzid_end != 0 {
                    return Err(ScramError::Malformed("authorization identity is not supported"));
                }
                let header_len = 1 + 1 + authzid_end + 1;
                (&msg[..header_len], &msg[header_len..])
            }
            Some(b'p') => return Err(ScramError::Malformed("channel binding is not supported")),
            _ => return Err(ScramError::Malformed("malformed GS2 header")),
        };

        let mut attrs = bare.split(',');
        let username = attrs.next().unwrap_or_default();
        if !username.starts_with("n=") {
            return Err(ScramError::Malformed("expected username attribute"));
        }
        let client_nonce = attrs
            .next()
            .and_then(|a| a.strip_prefix("r="))
            .ok_or(ScramError::Malformed("expected nonce attribute"))?;
        if !is_printable_nonce(client_nonce) {
            return Err(ScramError::Malformed("invalid client nonce"));
        }
        if !attrs.all(is_attribute) {
            return Err(ScramError::Malformed("malformed extension attribute"));
        }
        if !is_printable_nonce(server_nonce) {
            return Err(ScramError::Malformed("invalid server nonce"));
        }

        let nonce = format!("{}{}", client_nonce, server_nonce);
        let server_first = format!(
            "r={},s={},i={}",
            nonce,
            BASE64.encode(&self.salt),
            self.iterations
        );
        Ok((
            ScramExchange {
                gs2_header: gs2_header.to_string(),
                client_first_bare: bare.to_string(),
                server_first: server_first.clone(),
                nonce,
            },
            server_first,
        ))
    }

    /// Verify client-final-message; returns server-final-message on success.
    pub fn server_final(
        &self,
        exchange: &ScramExchange,
        client_final: &[u8],
    ) -> Result<String, ScramError> {
        let msg = std::str::from_utf8(client_final)
            .map_err(|_| ScramError::Malformed("client-final-message is not UTF-8"))?;
        let proof_at = msg
            .rfind(",p=")
            .ok_or(ScramError::Malformed("expected proof attribute"))?;
        let without_proof = &msg[..proof_at];
        let proof = BASE64
            .decode(&msg[proof_at + 3..])
            .map_err(|_| ScramError::Malformed("proof is not valid base64"))?;
        if proof.len() != 32 {
            return Err(ScramError::Malformed("proof has the wrong length"));
        }

        let mut attrs = without_proof.split(',');
        let binding = attrs
            .next()
            .and_then(|a| a.strip_prefix("c="))
            .ok_or(ScramError::Malformed("expected channel-binding attribute"))?;
        let binding = BASE64
            .decode(binding)
            .map_err(|_| ScramError::Malformed("channel-binding is not valid base64"))?;
        if binding != exchange.gs2_header.as_bytes() {
            return Err(ScramError::Malformed("channel-binding does not match the GS2 header"));
        }
        let nonce = attrs
            .next()
            .and_then(|a| a.strip_prefix("r="))
            .ok_or(ScramError::Malformed("expected nonce attribute"))?;
        if nonce != exchange.nonce {
            return Err(ScramError::Malformed("nonce does not match"));
        }

        let auth_message = format!(
            "{},{},{}",
            exchange.client_first_bare, exchange.server_first, without_proof
        );
        let client_signature = hmac(&self.stored_key, auth_message.as_bytes());
        let mut client_key = [0u8; 32];
        for (i, k) in client_key.iter_mut().enumerate() {
            *k = proof[i] ^ client_signature[i];
        }
        if !bool::from(sha256(&client_key).ct_eq(&self.stored_key)) {
            return Err(ScramError::InvalidProof);
        }

        let server_signature = hmac(&self.server_key, auth_message.as_bytes());
        Ok(format!("v={}", BASE64.encode(server_signature)))
    }
}

/// Highest iteration count accepted from a server (RDS uses 4096).
const MAX_CLIENT_ITERATIONS: u32 = 1_000_000;

/// Client side of the exchange, used to sign in to RDS with a password.
/// No channel binding ("n,,"), so "SCRAM-SHA-256" is chosen even when the
/// server also offers "SCRAM-SHA-256-PLUS".
pub struct ScramClient {
    password: String,
    client_first_bare: String,
    client_nonce: String,
    server_signature: Option<[u8; 32]>,
}

impl ScramClient {
    /// Returns the client and its client-first-message.
    pub fn new(password: &str, client_nonce: &str) -> (Self, String) {
        let client_first_bare = format!("n=,r={}", client_nonce);
        let first = format!("n,,{}", client_first_bare);
        (
            Self {
                password: password.to_string(),
                client_first_bare,
                client_nonce: client_nonce.to_string(),
                server_signature: None,
            },
            first,
        )
    }

    /// Handle server-first-message; returns client-final-message.
    pub fn client_final(&mut self, server_first: &[u8]) -> Result<String, ScramError> {
        let server_first = std::str::from_utf8(server_first)
            .map_err(|_| ScramError::Malformed("server-first-message is not UTF-8"))?;
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for attr in server_first.split(',') {
            if let Some(v) = attr.strip_prefix("r=") {
                nonce = Some(v);
            } else if let Some(v) = attr.strip_prefix("s=") {
                salt = Some(
                    BASE64
                        .decode(v)
                        .map_err(|_| ScramError::Malformed("salt is not valid base64"))?,
                );
            } else if let Some(v) = attr.strip_prefix("i=") {
                iterations = v.parse::<u32>().ok();
            }
        }
        let nonce = nonce.ok_or(ScramError::Malformed("expected nonce attribute"))?;
        if !nonce.starts_with(&self.client_nonce)
            || nonce.len() == self.client_nonce.len()
            || !is_printable_nonce(nonce)
        {
            return Err(ScramError::Malformed("server nonce does not extend the client nonce"));
        }
        let salt = salt.ok_or(ScramError::Malformed("expected salt attribute"))?;
        let iterations = iterations
            .filter(|i| (1..=MAX_CLIENT_ITERATIONS).contains(i))
            .ok_or(ScramError::Malformed("invalid iteration count"))?;

        let without_proof = format!("c=biws,r={}", nonce);
        let salted = hi(self.password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = sha256(&client_key);
        let auth_message = format!(
            "{},{},{}",
            self.client_first_bare, server_first, without_proof
        );
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(signature).map(|(k, s)| k ^ s).collect();
        self.server_signature = Some(hmac(
            &hmac(&salted, b"Server Key"),
            auth_message.as_bytes(),
        ));
        Ok(format!("{},p={}", without_proof, BASE64.encode(proof)))
    }

    /// Check server-final-message: proves the server knows the password too.
    pub fn verify_server_final(&self, server_final: &[u8]) -> Result<(), ScramError> {
        let expected = self
            .server_signature
            .ok_or(ScramError::Malformed("server-final-message before server-first-message"))?;
        let server_final = std::str::from_utf8(server_final)
            .map_err(|_| ScramError::Malformed("server-final-message is not UTF-8"))?;
        let signature = server_final
            .split(',')
            .find_map(|a| a.strip_prefix("v="))
            .ok_or(ScramError::Malformed("server-final-message has no verifier"))?;
        let signature = BASE64
            .decode(signature)
            .map_err(|_| ScramError::Malformed("server signature is not valid base64"))?;
        if bool::from(signature.as_slice().ct_eq(&expected)) {
            Ok(())
        } else {
            Err(ScramError::InvalidProof)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC7677_SALT: &str = "W22ZaJ0SNY7soEsUEjb6gQ==";
    const RFC7677_SERVER_NONCE: &str = "%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";

    fn rfc_verifier() -> ScramVerifier {
        ScramVerifier::new("pencil", BASE64.decode(RFC7677_SALT).unwrap(), 4096)
    }

    #[test]
    fn rfc7677_test_vector() {
        let verifier = rfc_verifier();
        let (exchange, server_first) = verifier
            .server_first(b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO", RFC7677_SERVER_NONCE)
            .unwrap();
        assert_eq!(
            server_first,
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"
        );
        let server_final = verifier
            .server_final(
                &exchange,
                b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=",
            )
            .unwrap();
        assert_eq!(server_final, "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
    }

    #[test]
    fn rfc7677_test_vector_client_side() {
        let (mut client, first) = ScramClient::new("pencil", "rOprNGfwEbeRWgbNEkqO");
        assert_eq!(first, "n,,n=,r=rOprNGfwEbeRWgbNEkqO");
        // The RFC exchange names the user ("n=user"); PostgreSQL clients send "n=".
        client.client_first_bare = "n=user,r=rOprNGfwEbeRWgbNEkqO".to_string();
        let client_final = client
            .client_final(b"r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")
            .unwrap();
        assert_eq!(
            client_final,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        assert!(client
            .verify_server_final(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .is_ok());
        assert_eq!(
            client.verify_server_final(b"v=AAAATRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="),
            Err(ScramError::InvalidProof)
        );
    }

    fn exchange(server_pw: &str, client_pw: &str) -> Result<(), ScramError> {
        let verifier = ScramVerifier::new(server_pw, b"0123456789abcdef".to_vec(), 4096);
        let (mut client, first) = ScramClient::new(client_pw, "clientnonce");
        let (exchange, server_first) = verifier.server_first(first.as_bytes(), "servernonce")?;
        let client_final = client.client_final(server_first.as_bytes())?;
        let server_final = verifier.server_final(&exchange, client_final.as_bytes())?;
        client.verify_server_final(server_final.as_bytes())
    }

    #[test]
    fn client_and_server_agree_on_the_right_password_only() {
        assert_eq!(exchange("right-password", "right-password"), Ok(()));
        assert_eq!(exchange("right-password", "wrong-password"), Err(ScramError::InvalidProof));
    }

    #[test]
    fn client_rejects_a_bad_server_first_message() {
        for bad in [
            &b"r=othernonce123,s=c2FsdA==,i=4096"[..],
            b"r=clientnonce,s=c2FsdA==,i=4096",
            b"r=clientnonceX,s=!!!,i=4096",
            b"r=clientnonceX,s=c2FsdA==,i=0",
            b"r=clientnonceX,s=c2FsdA==,i=99999999",
            b"r=clientnonceX,i=4096",
        ] {
            let (mut client, _) = ScramClient::new("pw", "clientnonce");
            assert!(client.client_final(bad).is_err(), "accepted {:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn rejects_malformed_client_first() {
        let verifier = rfc_verifier();
        for bad in [
            &b"p=tls-server-end-point,,n=,r=abc"[..],
            b"n,a=admin,n=,r=abc",
            b"x,,n=,r=abc",
            b"n,,r=abc",
            b"n,,n=,r=",
            b"n,,n=,r=a,b",
            b"n,",
            b"\xff\xfe",
        ] {
            assert!(
                verifier.server_first(bad, "srv").is_err(),
                "accepted {:?}",
                String::from_utf8_lossy(bad)
            );
        }
        // "y" (client supports binding, server did not offer it) is fine
        assert!(verifier.server_first(b"y,,n=,r=abc", "srv").is_ok());
    }

    #[test]
    fn rejects_tampered_client_final() {
        let verifier = ScramVerifier::new("pw", b"saltsaltsaltsalt".to_vec(), 4096);
        let (exchange, server_first) = verifier.server_first(b"n,,n=,r=abc", "def").unwrap();
        let (mut client, _) = ScramClient::new("pw", "abc");
        let good = client.client_final(server_first.as_bytes()).unwrap();

        let wrong_nonce = good.replace("r=abcdef", "r=abcdeX");
        assert_eq!(
            verifier.server_final(&exchange, wrong_nonce.as_bytes()),
            Err(ScramError::Malformed("nonce does not match"))
        );
        let wrong_binding = good.replace("c=biws", "c=eSws");
        assert!(matches!(
            verifier.server_final(&exchange, wrong_binding.as_bytes()),
            Err(ScramError::Malformed(_))
        ));
        assert!(verifier.server_final(&exchange, b"c=biws,r=abcdef").is_err());
        assert!(verifier.server_final(&exchange, b"c=biws,r=abcdef,p=AAAA").is_err());
    }
}
