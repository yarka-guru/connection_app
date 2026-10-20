//! PostgreSQL auth-injecting proxy for RDS.
//!
//! Each local client signs in with a stable local password (SCRAM-SHA-256 on
//! loopback). The proxy then opens a tunnel connection, does TLS to RDS
//! (verified against the embedded RDS CA bundle), signs in as the project's
//! database user and relays the rest of the session untouched.
//!
//! Upstream credentials: `authType: "iam"` always answers with an IAM token.
//! `authType: "secrets"` answers whatever RDS asks for: SCRAM or MD5 with the
//! secret's password, and a cleartext (PAM, i.e. IAM-enabled user) request
//! with an IAM token first and the password second.

pub mod scram;
pub mod wire;

#[cfg(test)]
mod tests;

use crate::aws::iam_auth::RdsAuthTokenCache;
use crate::tunnel::native::{accept_on, tune_local_socket, PipeInbox, TunnelPipe};
use futures_util::future::BoxFuture;
use md5::{Digest, Md5};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, Join};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use wire::{Message, StartupMessage, StartupPacket};

/// AWS RDS global CA bundle (https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem).
const RDS_CA_BUNDLE: &[u8] = include_bytes!("../../../certs/rds-global-bundle.pem");

/// Matches PostgreSQL's default authentication_timeout.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
/// Buffer of each in-memory pipe between the proxy and the tunnel.
const PIPE_SIZE: usize = 64 * 1024;
const PIPE_QUEUE: usize = 64;
const MAX_ENCRYPTION_REQUESTS: usize = 2;

/// Which credential answers a cleartext password request first in auto mode.
/// RDS asks for a cleartext password only for users in `rds_iam` (PAM); password
/// users get SCRAM or MD5. So the token is right first time on IAM databases and
/// the password is never tried there; flip this to try the password first.
const CLEARTEXT_FIRST: Credential = Credential::IamToken;

/// One tunnel connection to RDS, as seen by the proxy.
pub type Upstream = Join<DuplexStream, DuplexStream>;

/// Opens a fresh connection to RDS through the tunnel.
pub trait Dial: Send + Sync {
    fn dial(&self) -> BoxFuture<'_, std::io::Result<Upstream>>;
}

/// Hands in-memory pipes to whichever tunnel session is running.
pub struct TunnelDialer {
    tx: mpsc::Sender<TunnelPipe>,
}

impl Dial for TunnelDialer {
    fn dial(&self) -> BoxFuture<'_, std::io::Result<Upstream>> {
        Box::pin(async move {
            let (tunnel_reader, proxy_writer) = tokio::io::duplex(PIPE_SIZE);
            let (tunnel_writer, proxy_reader) = tokio::io::duplex(PIPE_SIZE);
            self.tx
                .send(TunnelPipe {
                    reader: tunnel_reader,
                    writer: tunnel_writer,
                })
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the tunnel is not running")
                })?;
            Ok(tokio::io::join(proxy_reader, proxy_writer))
        })
    }
}

/// A dialer for the proxy and the inbox the tunnel sessions drain.
pub fn tunnel_pipes() -> (TunnelDialer, PipeInbox) {
    let (tx, rx) = mpsc::channel(PIPE_QUEUE);
    (TunnelDialer { tx }, Arc::new(tokio::sync::Mutex::new(rx)))
}

/// Source of IAM auth tokens.
pub trait TokenSource: Send + Sync {
    fn token(&self) -> BoxFuture<'_, Result<String, String>>;
    fn invalidate(&self) -> BoxFuture<'_, ()>;
}

impl TokenSource for RdsAuthTokenCache {
    fn token(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move { RdsAuthTokenCache::token(self).await.map_err(|e| e.to_string()) })
    }

    fn invalidate(&self) -> BoxFuture<'_, ()> {
        Box::pin(RdsAuthTokenCache::invalidate(self))
    }
}

/// How the proxy signs in to RDS.
pub enum UpstreamAuth {
    /// `authType: "iam"`: always an IAM token.
    IamOnly,
    /// `authType: "secrets"`: decided per connection by what RDS asks for.
    Auto { password: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Credential {
    IamToken,
    Password,
}

impl Credential {
    fn other(self) -> Self {
        match self {
            Credential::IamToken => Credential::Password,
            Credential::Password => Credential::IamToken,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Credential::IamToken => "IAM token",
            Credential::Password => "secret password",
        }
    }
}

/// Settings for one RDS connection (project + profile).
pub struct ProxySettings {
    pub db_user: String,
    pub local_password: String,
    pub auth: UpstreamAuth,
    pub sdk_config: aws_config::SdkConfig,
}

pub struct PgAuthProxy {
    db_user: String,
    verifier: scram::ScramVerifier,
    auth: UpstreamAuth,
    tokens: Arc<dyn TokenSource>,
    tls: TlsConnector,
    server_name: ServerName<'static>,
    dialer: Arc<dyn Dial>,
    cleartext_choice: Mutex<Credential>,
    last_method: Mutex<Option<Credential>>,
}

/// Parsed RDS CA bundle, shared by every proxy.
pub fn rds_tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(RDS_CA_BUNDLE) {
                let cert = cert.map_err(|e| format!("Invalid RDS CA bundle: {}", e))?;
                roots
                    .add(cert)
                    .map_err(|e| format!("Invalid RDS CA certificate: {}", e))?;
            }
            client_tls_config(roots)
        })
        .clone()
}

fn client_tls_config(roots: rustls::RootCertStore) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup failed: {}", e))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| format!("No secure randomness available: {}", e))?;
    Ok(buf)
}

fn random_nonce() -> Result<String, Failure> {
    let bytes = random_bytes::<18>().map_err(|e| Failure::reject("XX000", e))?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        bytes,
    ))
}

/// PostgreSQL's MD5 password response: "md5" + md5(md5(password + user) + salt).
fn md5_response(user: &str, password: &str, salt: &[u8]) -> String {
    let inner = hex(&Md5::digest([password.as_bytes(), user.as_bytes()].concat()));
    let outer = hex(&Md5::digest([inner.as_bytes(), salt].concat()));
    format!("md5{}", outer)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn is_auth_error(sqlstate: &str) -> bool {
    sqlstate.starts_with("28")
}

/// Why a handshake stopped.
#[derive(Debug)]
enum Failure {
    /// Report to the client as a FATAL ErrorResponse.
    Reject {
        sqlstate: &'static str,
        message: String,
        hint: Option<&'static str>,
    },
    /// RDS refused the sign-in; `response` is its ErrorResponse, relayed as is.
    Refused {
        response: Vec<u8>,
        sqlstate: String,
        cleartext: Option<Credential>,
    },
    /// The client went away or broke the protocol; nothing to report.
    Closed(String),
}

impl Failure {
    fn reject(sqlstate: &'static str, message: impl Into<String>) -> Self {
        Failure::Reject {
            sqlstate,
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(self, hint: &'static str) -> Self {
        match self {
            Failure::Reject { sqlstate, message, .. } => Failure::Reject {
                sqlstate,
                message,
                hint: Some(hint),
            },
            other => other,
        }
    }

    fn client_io(e: std::io::Error) -> Self {
        Failure::Closed(format!("client connection: {}", e))
    }

    fn upstream_io(e: std::io::Error) -> Self {
        Failure::reject(
            "08006",
            format!("connection to RDS through the SSM tunnel failed: {}", e),
        )
    }

    fn describe(&self) -> String {
        match self {
            Failure::Reject { message, .. } => message.clone(),
            Failure::Refused { response, .. } => {
                wire::error_field(response.get(5..).unwrap_or_default(), b'M').unwrap_or_default()
            }
            Failure::Closed(reason) => reason.clone(),
        }
    }
}

impl PgAuthProxy {
    /// Proxy for one RDS endpoint, trusting only the embedded RDS CA bundle.
    pub fn for_rds(
        settings: ProxySettings,
        rds_host: &str,
        rds_port: u16,
        dialer: Arc<dyn Dial>,
    ) -> Result<Self, String> {
        let tokens = Arc::new(RdsAuthTokenCache::new(
            settings.sdk_config,
            rds_host,
            rds_port,
            &settings.db_user,
        ));
        Self::new(
            &settings.db_user,
            &settings.local_password,
            settings.auth,
            tokens,
            rds_tls_config()?,
            rds_host,
            dialer,
        )
    }

    pub fn new(
        db_user: &str,
        local_password: &str,
        auth: UpstreamAuth,
        tokens: Arc<dyn TokenSource>,
        tls: Arc<rustls::ClientConfig>,
        upstream_host: &str,
        dialer: Arc<dyn Dial>,
    ) -> Result<Self, String> {
        let server_name = ServerName::try_from(upstream_host.to_string())
            .map_err(|e| format!("Invalid RDS host name {}: {}", upstream_host, e))?;
        Ok(Self {
            db_user: db_user.to_string(),
            verifier: scram::ScramVerifier::new(
                local_password,
                random_bytes::<16>()?.to_vec(),
                scram::DEFAULT_ITERATIONS,
            ),
            auth,
            tokens,
            tls: TlsConnector::from(tls),
            server_name,
            dialer,
            cleartext_choice: Mutex::new(CLEARTEXT_FIRST),
            last_method: Mutex::new(None),
        })
    }

    /// Auth method of the last successful upstream sign-in, for display:
    /// "IAM", "IAM (auto)" or "password".
    pub fn method_label(&self) -> Option<String> {
        let method = *self.last_method.lock().unwrap_or_else(|e| e.into_inner());
        method.map(|m| match (m, &self.auth) {
            (Credential::IamToken, UpstreamAuth::IamOnly) => "IAM".to_string(),
            (Credential::IamToken, UpstreamAuth::Auto { .. }) => "IAM (auto)".to_string(),
            (Credential::Password, _) => "password".to_string(),
        })
    }

    fn cleartext_choice(&self) -> Credential {
        match self.auth {
            UpstreamAuth::IamOnly => Credential::IamToken,
            UpstreamAuth::Auto { .. } => *self.cleartext_choice.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Sign in to RDS once and log out, so the method is known (and any
    /// credential problem shows) before a client connects.
    pub async fn probe(&self, database: &str) -> Result<String, String> {
        let mut params = vec![(b"user".to_vec(), self.db_user.as_bytes().to_vec())];
        if !database.is_empty() {
            params.push((b"database".to_vec(), database.as_bytes().to_vec()));
        }
        params.push((b"application_name".to_vec(), b"ConnectionApp auth check".to_vec()));
        let startup = StartupMessage {
            protocol: 196608,
            params,
        };
        let (mut rds, _) = self.sign_in(&startup).await.map_err(|f| f.describe())?;
        let _ = rds.write_all(&wire::encode_message(b'X', &[])).await;
        let _ = rds.shutdown().await;
        self.method_label()
            .ok_or_else(|| "sign-in finished without an auth method".to_string())
    }

    /// Serve one local client.
    pub async fn serve<C>(self: Arc<Self>, mut client: C)
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let outcome = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.handshake(&mut client)).await;
        let failure = match outcome {
            Ok(Ok(Some(mut rds))) => {
                if let Err(e) = tokio::io::copy_bidirectional(&mut client, &mut rds).await {
                    log::debug!("Auth proxy: session ended: {}", e);
                }
                return;
            }
            Ok(Ok(None)) => return,
            Ok(Err(failure)) => failure,
            Err(_) => Failure::reject("08006", "timed out while signing in to RDS"),
        };
        let response = match failure {
            Failure::Reject {
                sqlstate,
                message,
                hint,
            } => {
                log::warn!("Auth proxy: rejected local client: {} ({})", message, sqlstate);
                wire::error_response(sqlstate, &message, hint)
            }
            Failure::Refused { response, .. } => response,
            Failure::Closed(reason) => {
                log::debug!("Auth proxy: connection closed: {}", reason);
                return;
            }
        };
        let _ = client.write_all(&response).await;
        let _ = client.flush().await;
        let _ = client.shutdown().await;
    }

    async fn handshake<C>(&self, client: &mut C) -> Result<Option<TlsStream<Upstream>>, Failure>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let mut encryption_requests = 0;
        let startup = loop {
            let packet = wire::read_startup_packet(client).await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    Failure::reject("08P01", e.to_string())
                } else {
                    Failure::client_io(e)
                }
            })?;
            match packet {
                StartupPacket::SslRequest | StartupPacket::GssEncRequest => {
                    encryption_requests += 1;
                    if encryption_requests > MAX_ENCRYPTION_REQUESTS {
                        return Err(Failure::reject("08P01", "too many encryption requests"));
                    }
                    client.write_all(b"N").await.map_err(Failure::client_io)?;
                    client.flush().await.map_err(Failure::client_io)?;
                }
                StartupPacket::CancelRequest(packet) => {
                    self.forward_cancel(&packet).await;
                    return Ok(None);
                }
                StartupPacket::Startup(msg) => break msg,
            }
        };

        let user = startup
            .param("user")
            .map(|u| String::from_utf8_lossy(u).into_owned())
            .ok_or_else(|| Failure::reject("28000", "no PostgreSQL user name specified in startup packet"))?;

        self.authenticate_client(client, &user).await?;

        if user != self.db_user {
            return Err(Failure::reject(
                "28000",
                format!(
                    "this ConnectionApp connection only signs in as \"{}\", not \"{}\"",
                    self.db_user, user
                ),
            )
            .with_hint("Set the user name in your database client to the one ConnectionApp shows."));
        }

        let (rds, relay) = self.sign_in(&startup).await?;
        client.write_all(&relay).await.map_err(Failure::client_io)?;
        client.flush().await.map_err(Failure::client_io)?;
        Ok(Some(rds))
    }

    async fn authenticate_client<C>(&self, client: &mut C, user: &str) -> Result<(), Failure>
    where
        C: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let mechanisms = format!("{}\0\0", scram::MECHANISM);
        client
            .write_all(&wire::auth_request(wire::AUTH_SASL, mechanisms.as_bytes()))
            .await
            .map_err(Failure::client_io)?;
        client.flush().await.map_err(Failure::client_io)?;

        let initial = read_password_message(client).await?;
        let (mechanism, client_first) =
            wire::parse_sasl_initial_response(&initial.body).map_err(|e| Failure::reject("08P01", e.to_string()))?;
        if mechanism != scram::MECHANISM.as_bytes() {
            return Err(Failure::reject(
                "28000",
                "client selected an invalid SASL authentication mechanism",
            ));
        }

        let (exchange, server_first) = self
            .verifier
            .server_first(client_first, &random_nonce()?)
            .map_err(|e| scram_failure(e, user))?;
        client
            .write_all(&wire::auth_request(wire::AUTH_SASL_CONTINUE, server_first.as_bytes()))
            .await
            .map_err(Failure::client_io)?;
        client.flush().await.map_err(Failure::client_io)?;

        let response = read_password_message(client).await?;
        let server_final = self
            .verifier
            .server_final(&exchange, &response.body)
            .map_err(|e| scram_failure(e, user))?;
        client
            .write_all(&wire::auth_request(wire::AUTH_SASL_FINAL, server_final.as_bytes()))
            .await
            .map_err(Failure::client_io)?;
        client.flush().await.map_err(Failure::client_io)?;
        Ok(())
    }

    /// Sign in upstream; in auto mode a refused cleartext answer is retried
    /// once with the other credential on a fresh connection, and the one that
    /// worked is remembered for later connections.
    async fn sign_in(&self, startup: &StartupMessage) -> Result<(TlsStream<Upstream>, Vec<u8>), Failure> {
        let first = self.cleartext_choice();
        match self.attempt(startup, first).await {
            Err(Failure::Refused {
                cleartext: Some(used),
                ref sqlstate,
                ..
            }) if matches!(self.auth, UpstreamAuth::Auto { .. }) && is_auth_error(sqlstate) => {
                let other = used.other();
                log::info!(
                    "Auth proxy: RDS refused the {} for {}, retrying with the {}",
                    used.describe(),
                    self.db_user,
                    other.describe()
                );
                let result = self.attempt(startup, other).await?;
                *self.cleartext_choice.lock().unwrap_or_else(|e| e.into_inner()) = other;
                Ok(result)
            }
            result => result,
        }
    }

    async fn attempt(
        &self,
        startup: &StartupMessage,
        cleartext: Credential,
    ) -> Result<(TlsStream<Upstream>, Vec<u8>), Failure> {
        let upstream = self.dialer.dial().await.map_err(Failure::upstream_io)?;
        let mut rds = self.connect_tls(upstream).await?;
        let relay = self.authenticate_upstream(&mut rds, startup, cleartext).await?;
        Ok((rds, relay))
    }

    async fn connect_tls<U>(&self, mut upstream: U) -> Result<TlsStream<U>, Failure>
    where
        U: AsyncRead + AsyncWrite + Unpin + Send,
    {
        upstream
            .write_all(&wire::ssl_request())
            .await
            .map_err(Failure::upstream_io)?;
        upstream.flush().await.map_err(Failure::upstream_io)?;
        let mut answer = [0u8; 1];
        upstream
            .read_exact(&mut answer)
            .await
            .map_err(Failure::upstream_io)?;
        match answer[0] {
            b'S' => {}
            b'N' => {
                return Err(Failure::reject(
                    "08001",
                    "RDS refused TLS; ConnectionApp does not send database credentials without TLS",
                ));
            }
            _ => return Err(Failure::reject("08P01", "unexpected reply from RDS to SSLRequest")),
        }
        self.tls
            .connect(self.server_name.clone(), upstream)
            .await
            .map_err(|e| Failure::reject("08001", format!("TLS handshake with RDS failed: {}", e)))
    }

    fn secret_password(&self) -> Option<&str> {
        match &self.auth {
            UpstreamAuth::Auto { password } => Some(password),
            UpstreamAuth::IamOnly => None,
        }
    }

    /// Run the upstream authentication exchange. Returns the bytes to relay
    /// to the client (any notices and the AuthenticationOk).
    async fn authenticate_upstream<U>(
        &self,
        rds: &mut TlsStream<U>,
        startup: &StartupMessage,
        cleartext: Credential,
    ) -> Result<Vec<u8>, Failure>
    where
        U: AsyncRead + AsyncWrite + Unpin + Send,
    {
        rds.write_all(&startup.with_user(&self.db_user).encode())
            .await
            .map_err(Failure::upstream_io)?;
        rds.flush().await.map_err(Failure::upstream_io)?;

        let mut relay = Vec::new();
        let mut used: Option<Credential> = None;
        let mut cleartext_used: Option<Credential> = None;
        let mut scram_client: Option<scram::ScramClient> = None;
        loop {
            let msg = wire::read_message(rds, wire::MAX_AUTH_MESSAGE_LEN)
                .await
                .map_err(Failure::upstream_io)?;
            let reply = match (msg.tag, msg.auth_code()) {
                (b'R', Some(wire::AUTH_OK)) => {
                    relay.extend_from_slice(&msg.encode());
                    *self.last_method.lock().unwrap_or_else(|e| e.into_inner()) = used;
                    log::debug!(
                        "Auth proxy: signed in to RDS as {} with the {}",
                        self.db_user,
                        used.map(Credential::describe).unwrap_or("server's trust")
                    );
                    return Ok(relay);
                }
                (b'R', Some(wire::AUTH_CLEARTEXT_PASSWORD)) if used.is_none() => {
                    let credential = match self.secret_password() {
                        Some(password) if cleartext == Credential::Password => {
                            used = Some(Credential::Password);
                            password.to_string()
                        }
                        _ => {
                            used = Some(Credential::IamToken);
                            self.tokens.token().await.map_err(|e| {
                                Failure::reject(
                                    "28000",
                                    format!("ConnectionApp could not create an RDS IAM auth token: {}", e),
                                )
                                .with_hint("Sign in to AWS again (SSO) in ConnectionApp, then reconnect.")
                            })?
                        }
                    };
                    cleartext_used = used;
                    wire::password_message(&credential)
                }
                (b'R', Some(wire::AUTH_MD5_PASSWORD)) if used.is_none() && msg.body.len() == 8 => {
                    let password = self.secret_password().ok_or_else(|| iam_only_mismatch(wire::AUTH_MD5_PASSWORD))?;
                    used = Some(Credential::Password);
                    let response = md5_response(&self.db_user, password, &msg.body[4..8]);
                    wire::password_message(&response)
                }
                (b'R', Some(wire::AUTH_SASL)) if used.is_none() => {
                    let password = self.secret_password().ok_or_else(|| iam_only_mismatch(wire::AUTH_SASL))?;
                    if !wire::sasl_mechanisms(&msg.body[4..]).any(|m| m == scram::MECHANISM.as_bytes()) {
                        return Err(Failure::reject("28000", "RDS offered no SASL mechanism ConnectionApp supports"));
                    }
                    used = Some(Credential::Password);
                    let (client, first) = scram::ScramClient::new(password, &random_nonce()?);
                    scram_client = Some(client);
                    wire::sasl_initial_response(scram::MECHANISM, first.as_bytes())
                }
                (b'R', Some(wire::AUTH_SASL_CONTINUE)) if scram_client.is_some() => {
                    let client = scram_client.as_mut().expect("checked above");
                    let client_final = client
                        .client_final(&msg.body[4..])
                        .map_err(|e| Failure::reject("08P01", format!("bad SCRAM message from RDS: {:?}", e)))?;
                    wire::encode_message(b'p', client_final.as_bytes())
                }
                (b'R', Some(wire::AUTH_SASL_FINAL)) if scram_client.is_some() => {
                    let client = scram_client.as_ref().expect("checked above");
                    client.verify_server_final(&msg.body[4..]).map_err(|_| {
                        Failure::reject("08006", "RDS failed SCRAM server verification")
                    })?;
                    continue;
                }
                (b'R', code) => {
                    let code = code.unwrap_or_default();
                    return Err(match self.auth {
                        UpstreamAuth::IamOnly => iam_only_mismatch(code),
                        UpstreamAuth::Auto { .. } => Failure::reject(
                            "28000",
                            format!("RDS asked for unsupported authentication method {}", code),
                        ),
                    });
                }
                (b'E', _) => {
                    let sqlstate = wire::error_field(&msg.body, b'C').unwrap_or_default();
                    let text = wire::error_field(&msg.body, b'M').unwrap_or_default();
                    log::warn!(
                        "Auth proxy: RDS refused sign-in as {} with the {}: {} ({})",
                        self.db_user,
                        used.map(Credential::describe).unwrap_or("startup packet"),
                        text,
                        sqlstate
                    );
                    if used == Some(Credential::IamToken) && is_auth_error(&sqlstate) {
                        self.tokens.invalidate().await;
                    }
                    return Err(Failure::Refused {
                        response: msg.encode(),
                        sqlstate,
                        cleartext: cleartext_used,
                    });
                }
                (b'N', _) | (b'v', _) => {
                    relay.extend_from_slice(&msg.encode());
                    continue;
                }
                (tag, _) => {
                    return Err(Failure::reject(
                        "08P01",
                        format!("unexpected message '{}' from RDS during sign-in", tag as char),
                    ));
                }
            };
            rds.write_all(&reply).await.map_err(Failure::upstream_io)?;
            rds.flush().await.map_err(Failure::upstream_io)?;
        }
    }

    async fn forward_cancel(&self, packet: &[u8]) {
        let result = tokio::time::timeout(CANCEL_TIMEOUT, async {
            let upstream = self.dialer.dial().await.map_err(|e| e.to_string())?;
            let mut rds = self.connect_tls(upstream).await.map_err(|f| f.describe())?;
            rds.write_all(packet).await.map_err(|e| e.to_string())?;
            rds.flush().await.map_err(|e| e.to_string())?;
            let mut sink = [0u8; 64];
            let _ = rds.read(&mut sink).await;
            Ok::<(), String>(())
        })
        .await;
        match result {
            Ok(Ok(())) => log::debug!("Auth proxy: forwarded a cancel request to RDS"),
            Ok(Err(e)) => log::warn!("Auth proxy: could not forward a cancel request: {}", e),
            Err(_) => log::warn!("Auth proxy: forwarding a cancel request timed out"),
        }
    }
}

fn iam_only_mismatch(code: u32) -> Failure {
    Failure::reject(
        "28000",
        format!("RDS asked for authentication method {} instead of an IAM token", code),
    )
    .with_hint("Check that the database user is granted the rds_iam role.")
}

fn scram_failure(e: scram::ScramError, user: &str) -> Failure {
    match e {
        scram::ScramError::InvalidProof => Failure::reject(
            "28P01",
            format!("password authentication failed for user \"{}\"", user),
        ),
        scram::ScramError::Malformed(what) => {
            Failure::reject("08P01", format!("malformed SCRAM message: {}", what))
        }
    }
}

async fn read_password_message<C: AsyncRead + Unpin>(client: &mut C) -> Result<Message, Failure> {
    let msg = wire::read_message(client, wire::MAX_AUTH_MESSAGE_LEN)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                Failure::reject("08P01", e.to_string())
            } else {
                Failure::client_io(e)
            }
        })?;
    if msg.tag != b'p' {
        return Err(Failure::reject(
            "08P01",
            format!("expected SASL response, got message type '{}'", msg.tag as char),
        ));
    }
    Ok(msg)
}

/// Accept local clients until cancelled; each is served on its own task.
pub async fn run_listener(
    proxy: Arc<PgAuthProxy>,
    listener_v4: TcpListener,
    listener_v6: Option<TcpListener>,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            result = accept_on(Some(&listener_v4)) => result,
            result = accept_on(listener_v6.as_ref()) => result,
            _ = cancel.cancelled() => break,
        };
        match accepted {
            Ok((stream, _addr)) => {
                tune_local_socket(&stream);
                tokio::spawn(proxy.clone().serve::<TcpStream>(stream));
            }
            Err(e) => log::error!("Auth proxy: accept error: {}", e),
        }
    }
}
