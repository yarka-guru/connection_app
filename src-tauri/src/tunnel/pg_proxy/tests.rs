use super::*;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::DuplexStream;
use tokio_rustls::TlsAcceptor;

const RDS_HOST: &str = "db.test.internal";
const DB_USER: &str = "covered_app";
const LOCAL_PASSWORD: &str = "LocalPassw0rdForTests1234";
const SECRET: &str = "secret-db-password";
const TOKEN: &str = "db.test.internal:5432/?Action=connect&DBUser=covered_app&X-Amz-Signature=test";

struct StaticTokens {
    issued: AtomicUsize,
    invalidated: AtomicUsize,
}

impl TokenSource for StaticTokens {
    fn token(&self) -> BoxFuture<'_, Result<String, String>> {
        self.issued.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(TOKEN.to_string()) })
    }

    fn invalidate(&self) -> BoxFuture<'_, ()> {
        self.invalidated.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

/// How the fake RDS server behaves on one connection.
#[derive(Clone)]
enum Rds {
    /// Cleartext request (PAM, as for `rds_iam` users); accepts one password.
    Pam(&'static str),
    /// SCRAM-SHA-256 with this password, offering -PLUS too like RDS over TLS.
    Scram(&'static str),
    /// MD5 with this password.
    Md5(&'static str),
    RefuseTls,
}

/// What the fake RDS server saw on one connection.
#[derive(Debug, Default)]
struct Seen {
    ssl_requested: bool,
    startup: Option<StartupMessage>,
    cleartext: Option<String>,
    signed_in: bool,
    cancel: Option<Vec<u8>>,
    query: Option<Vec<u8>>,
    terminated: bool,
}

struct TestPki {
    client: Arc<rustls::ClientConfig>,
    acceptor: TlsAcceptor,
}

fn test_pki() -> TestPki {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_cert = rcgen::CertificateParams::new(vec![RDS_HOST.to_string()])
        .unwrap()
        .signed_by(&server_key, &ca)
        .unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.der().clone()).unwrap();

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![server_cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
        )
        .unwrap();

    TestPki {
        client: client_tls_config(roots).unwrap(),
        acceptor: TlsAcceptor::from(Arc::new(server)),
    }
}

/// Each dial gets the next scripted fake RDS server.
struct FakeTunnel {
    acceptor: TlsAcceptor,
    script: Mutex<VecDeque<Rds>>,
    seen: Arc<tokio::sync::Mutex<Vec<Seen>>>,
    dials: AtomicUsize,
}

impl Dial for FakeTunnel {
    fn dial(&self) -> BoxFuture<'_, std::io::Result<Upstream>> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let behaviour = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("more dials than scripted RDS connections");
        let (proxy_reader, rds_writer) = tokio::io::duplex(64 * 1024);
        let (rds_reader, proxy_writer) = tokio::io::duplex(64 * 1024);
        let acceptor = self.acceptor.clone();
        let seen = self.seen.clone();
        tokio::spawn(async move {
            let result = fake_rds(tokio::io::join(rds_reader, rds_writer), acceptor, behaviour).await;
            seen.lock().await.push(result);
        });
        Box::pin(async move { Ok(tokio::io::join(proxy_reader, proxy_writer)) })
    }
}

struct Harness {
    proxy: Arc<PgAuthProxy>,
    tunnel: Arc<FakeTunnel>,
    tokens: Arc<StaticTokens>,
}

impl Harness {
    fn new(pki: &TestPki, auth: UpstreamAuth, script: Vec<Rds>) -> Self {
        Self::with_host(pki, pki, auth, script, RDS_HOST)
    }

    fn with_host(server: &TestPki, trusted: &TestPki, auth: UpstreamAuth, script: Vec<Rds>, host: &str) -> Self {
        let tokens = Arc::new(StaticTokens {
            issued: AtomicUsize::new(0),
            invalidated: AtomicUsize::new(0),
        });
        let tunnel = Arc::new(FakeTunnel {
            acceptor: server.acceptor.clone(),
            script: Mutex::new(script.into()),
            seen: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            dials: AtomicUsize::new(0),
        });
        let proxy = Arc::new(
            PgAuthProxy::new(
                DB_USER,
                LOCAL_PASSWORD,
                auth,
                tokens.clone(),
                trusted.client.clone(),
                host,
                tunnel.clone(),
            )
            .unwrap(),
        );
        Self { proxy, tunnel, tokens }
    }

    fn client(&self) -> DuplexStream {
        let (client, proxy_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(self.proxy.clone().serve(proxy_side));
        client
    }

    fn dials(&self) -> usize {
        self.tunnel.dials.load(Ordering::SeqCst)
    }

    fn tokens_issued(&self) -> usize {
        self.tokens.issued.load(Ordering::SeqCst)
    }

    /// Wait until every dialed fake server has finished, then return what they saw.
    async fn seen(&self) -> Vec<Seen> {
        for _ in 0..200 {
            if self.tunnel.seen.lock().await.len() == self.dials() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::mem::take(&mut *self.tunnel.seen.lock().await)
    }
}

fn auto() -> UpstreamAuth {
    UpstreamAuth::Auto {
        password: SECRET.to_string(),
    }
}

async fn fake_rds(mut stream: Upstream, acceptor: TlsAcceptor, behaviour: Rds) -> Seen {
    let mut seen = Seen::default();
    let mut first = [0u8; 8];
    if stream.read_exact(&mut first).await.is_err() {
        return seen;
    }
    assert_eq!(first, wire::ssl_request(), "upstream must open with SSLRequest");
    seen.ssl_requested = true;
    if let Rds::RefuseTls = behaviour {
        let _ = stream.write_all(b"N").await;
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest).await;
        assert!(rest.is_empty(), "nothing may be sent after TLS was refused");
        return seen;
    }
    stream.write_all(b"S").await.unwrap();
    let Ok(mut tls) = acceptor.accept(stream).await else {
        return seen;
    };

    let startup = match wire::read_startup_packet(&mut tls).await.unwrap() {
        StartupPacket::CancelRequest(packet) => {
            seen.cancel = Some(packet);
            return seen;
        }
        StartupPacket::Startup(msg) => msg,
        other => panic!("unexpected packet {:?}", other),
    };
    seen.startup = Some(startup);

    let accepted = match behaviour {
        Rds::Pam(accept) => {
            send(&mut tls, &wire::auth_request(wire::AUTH_CLEARTEXT_PASSWORD, &[])).await;
            let msg = wire::read_message(&mut tls, 4096).await.unwrap();
            assert_eq!(msg.tag, b'p');
            let password = String::from_utf8(msg.body[..msg.body.len() - 1].to_vec()).unwrap();
            let ok = password == accept;
            seen.cleartext = Some(password);
            if !ok {
                send(&mut tls, &wire::error_response("28000", "PAM authentication failed for user \"covered_app\"", None)).await;
            }
            ok
        }
        Rds::Scram(password) => {
            let verifier = scram::ScramVerifier::new(password, b"rdssaltrdssalt12".to_vec(), 4096);
            send(&mut tls, &wire::auth_request(wire::AUTH_SASL, b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0")).await;
            let initial = wire::read_message(&mut tls, 4096).await.unwrap();
            let (mechanism, client_first) = wire::parse_sasl_initial_response(&initial.body).unwrap();
            assert_eq!(mechanism, b"SCRAM-SHA-256");
            let (exchange, server_first) = verifier.server_first(client_first, "rdsnonce").unwrap();
            send(&mut tls, &wire::auth_request(wire::AUTH_SASL_CONTINUE, server_first.as_bytes())).await;
            let response = wire::read_message(&mut tls, 4096).await.unwrap();
            match verifier.server_final(&exchange, &response.body) {
                Ok(server_final) => {
                    send(&mut tls, &wire::auth_request(wire::AUTH_SASL_FINAL, server_final.as_bytes())).await;
                    true
                }
                Err(_) => {
                    send(&mut tls, &wire::error_response("28P01", "password authentication failed for user \"covered_app\"", None)).await;
                    false
                }
            }
        }
        Rds::Md5(password) => {
            let salt = [1u8, 2, 3, 4];
            send(&mut tls, &wire::auth_request(wire::AUTH_MD5_PASSWORD, &salt)).await;
            let msg = wire::read_message(&mut tls, 4096).await.unwrap();
            let got = String::from_utf8(msg.body[..msg.body.len() - 1].to_vec()).unwrap();
            let ok = got == md5_response(DB_USER, password, &salt);
            if !ok {
                send(&mut tls, &wire::error_response("28P01", "password authentication failed", None)).await;
            }
            ok
        }
        Rds::RefuseTls => unreachable!(),
    };
    if !accepted {
        return seen;
    }
    seen.signed_in = true;

    let mut reply = wire::auth_request(wire::AUTH_OK, &[]);
    reply.extend_from_slice(&wire::encode_message(b'S', b"server_version\x0018.3\x00"));
    reply.extend_from_slice(&wire::encode_message(b'K', &[0, 0, 0, 42, 1, 2, 3, 4]));
    reply.extend_from_slice(&wire::encode_message(b'Z', b"I"));
    send(&mut tls, &reply).await;

    if let Ok(msg) = wire::read_message(&mut tls, 4096).await {
        if msg.tag == b'X' {
            seen.terminated = true;
        } else {
            let echo = wire::encode_message(b'D', &msg.body);
            seen.query = Some(msg.body);
            send(&mut tls, &echo).await;
        }
    }
    let _ = tls.shutdown().await;
    seen
}

async fn send<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) {
    w.write_all(bytes).await.unwrap();
    w.flush().await.unwrap();
}

fn startup(user: &str) -> Vec<u8> {
    StartupMessage {
        protocol: 196608,
        params: vec![
            (b"user".to_vec(), user.as_bytes().to_vec()),
            (b"database".to_vec(), b"covered_db".to_vec()),
            (b"application_name".to_vec(), b"proxy-test".to_vec()),
        ],
    }
    .encode()
}

async fn expect_auth(client: &mut DuplexStream, code: u32) -> Vec<u8> {
    let msg = wire::read_message(client, 4096).await.unwrap();
    assert_eq!(msg.auth_code(), Some(code), "unexpected message {:?}", msg);
    msg.body[4..].to_vec()
}

async fn expect_error(client: &mut DuplexStream) -> (String, String) {
    let msg = wire::read_message(client, 4096).await.unwrap();
    assert_eq!(msg.tag, b'E', "expected ErrorResponse, got {:?}", msg);
    (
        wire::error_field(&msg.body, b'C').unwrap(),
        wire::error_field(&msg.body, b'M').unwrap(),
    )
}

/// SSLRequest (declined), StartupMessage, full SCRAM exchange with the proxy.
async fn sign_in(client: &mut DuplexStream, user: &str, password: &str) -> Result<(), (String, String)> {
    send(client, &wire::ssl_request()).await;
    let mut answer = [0u8; 1];
    client.read_exact(&mut answer).await.unwrap();
    assert_eq!(&answer, b"N", "loopback hop must decline TLS");

    send(client, &startup(user)).await;
    let mechanisms = expect_auth(client, wire::AUTH_SASL).await;
    assert_eq!(mechanisms, b"SCRAM-SHA-256\0\0");

    let (mut scram_client, first) = scram::ScramClient::new(password, "testclientnonce");
    send(client, &wire::sasl_initial_response("SCRAM-SHA-256", first.as_bytes())).await;
    let server_first = expect_auth(client, wire::AUTH_SASL_CONTINUE).await;
    let client_final = scram_client.client_final(&server_first).unwrap();
    send(client, &wire::encode_message(b'p', client_final.as_bytes())).await;

    let msg = wire::read_message(client, 4096).await.unwrap();
    if msg.tag == b'E' {
        return Err((
            wire::error_field(&msg.body, b'C').unwrap(),
            wire::error_field(&msg.body, b'M').unwrap(),
        ));
    }
    assert_eq!(msg.auth_code(), Some(wire::AUTH_SASL_FINAL));
    scram_client
        .verify_server_final(&msg.body[4..])
        .expect("proxy's server signature must verify");
    Ok(())
}

/// Sign in, read the relayed startup tail and run one echoed query.
async fn full_session(client: &mut DuplexStream) {
    sign_in(client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    expect_auth(client, wire::AUTH_OK).await;
    assert_eq!(wire::read_message(client, 4096).await.unwrap().tag, b'S');
    let key_data = wire::read_message(client, 4096).await.unwrap();
    assert_eq!((key_data.tag, key_data.body.as_slice()), (b'K', &[0, 0, 0, 42, 1, 2, 3, 4][..]));
    assert_eq!(wire::read_message(client, 4096).await.unwrap().tag, b'Z');
    send(client, &wire::encode_message(b'Q', b"SELECT 1\0")).await;
    let echo = wire::read_message(client, 4096).await.unwrap();
    assert_eq!((echo.tag, echo.body.as_slice()), (b'D', &b"SELECT 1\0"[..]));
}

#[tokio::test]
async fn iam_project_signs_in_with_token_over_verified_tls_and_relays() {
    let pki = test_pki();
    let h = Harness::new(&pki, UpstreamAuth::IamOnly, vec![Rds::Pam(TOKEN)]);
    let mut client = h.client();
    full_session(&mut client).await;
    drop(client);

    let seen = h.seen().await;
    assert_eq!(seen.len(), 1);
    let startup = seen[0].startup.as_ref().unwrap();
    assert_eq!(startup.param("user"), Some(DB_USER.as_bytes()));
    assert_eq!(startup.param("database"), Some(&b"covered_db"[..]));
    assert_eq!(startup.param("application_name"), Some(&b"proxy-test"[..]));
    assert_eq!(seen[0].cleartext.as_deref(), Some(TOKEN));
    assert_eq!(seen[0].query.as_deref(), Some(&b"SELECT 1\0"[..]));
    assert_eq!(h.proxy.method_label().as_deref(), Some("IAM"));
}

#[tokio::test]
async fn wrong_local_password_is_rejected_before_anything_goes_upstream() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![]);
    let mut client = h.client();
    let err = sign_in(&mut client, DB_USER, "not-the-password").await.unwrap_err();
    assert_eq!(err.0, "28P01");
    assert_eq!(err.1, "password authentication failed for user \"covered_app\"");
    assert_eq!(h.dials(), 0, "nothing may reach RDS for a rejected client");
    assert_eq!(h.tokens_issued(), 0);
}

#[tokio::test]
async fn the_secret_password_is_not_accepted_locally() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![]);
    let mut client = h.client();
    let err = sign_in(&mut client, DB_USER, SECRET).await.unwrap_err();
    assert_eq!(err.0, "28P01");
    assert_eq!(h.dials(), 0);
}

#[tokio::test]
async fn other_user_names_are_rejected_after_the_password_check() {
    let pki = test_pki();
    let h = Harness::new(&pki, UpstreamAuth::IamOnly, vec![]);

    let mut client = h.client();
    assert!(sign_in(&mut client, "postgres", "wrong").await.is_err());

    let mut client = h.client();
    sign_in(&mut client, "postgres", LOCAL_PASSWORD).await.unwrap();
    let (code, message) = expect_error(&mut client).await;
    assert_eq!(code, "28000");
    assert!(message.contains("only signs in as \"covered_app\", not \"postgres\""), "{}", message);
    assert_eq!(h.dials(), 0);
}

#[tokio::test]
async fn certificate_for_another_host_fails_and_no_credential_is_sent() {
    let pki = test_pki();
    let h = Harness::with_host(&pki, &pki, UpstreamAuth::IamOnly, vec![Rds::Pam(TOKEN)], "other.test.internal");
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, message) = expect_error(&mut client).await;
    assert_eq!(code, "08001");
    assert!(message.contains("TLS handshake with RDS failed"), "{}", message);
    let seen = h.seen().await;
    assert!(seen[0].startup.is_none() && seen[0].cleartext.is_none());
    assert_eq!(h.tokens_issued(), 0);
}

#[tokio::test]
async fn untrusted_certificate_fails() {
    let pki = test_pki();
    let other = test_pki();
    let h = Harness::with_host(&pki, &other, auto(), vec![Rds::Scram(SECRET)], RDS_HOST);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, _) = expect_error(&mut client).await;
    assert_eq!(code, "08001");
    assert!(h.seen().await[0].startup.is_none());
}

#[tokio::test]
async fn rds_refusing_tls_never_gets_a_credential() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::RefuseTls]);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, _) = expect_error(&mut client).await;
    assert_eq!(code, "08001");
    let seen = h.seen().await;
    assert!(seen[0].ssl_requested && seen[0].cleartext.is_none());
}

#[tokio::test]
async fn iam_project_relays_a_refused_token_and_drops_it_without_retrying() {
    let pki = test_pki();
    let h = Harness::new(&pki, UpstreamAuth::IamOnly, vec![Rds::Pam("something-else")]);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, message) = expect_error(&mut client).await;
    assert_eq!(code, "28000");
    assert!(message.contains("PAM authentication failed"));
    assert_eq!(h.dials(), 1, "IAM-only projects never fall back");
    assert_eq!(h.tokens.invalidated.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn iam_project_refuses_password_auth_requests() {
    let pki = test_pki();
    let h = Harness::new(&pki, UpstreamAuth::IamOnly, vec![Rds::Scram(SECRET)]);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, message) = expect_error(&mut client).await;
    assert_eq!(code, "28000");
    assert!(message.contains("instead of an IAM token"), "{}", message);
}

#[tokio::test]
async fn secrets_project_uses_iam_when_rds_asks_for_pam() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Pam(TOKEN), Rds::Pam(TOKEN)]);
    for _ in 0..2 {
        let mut client = h.client();
        full_session(&mut client).await;
    }
    let seen = h.seen().await;
    assert_eq!(h.dials(), 2, "one connection per session, no failed attempts");
    assert!(seen.iter().all(|s| s.cleartext.as_deref() == Some(TOKEN)));
    assert_eq!(h.proxy.method_label().as_deref(), Some("IAM (auto)"));
}

#[tokio::test]
async fn secrets_project_uses_the_password_for_scram_without_minting_a_token() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Scram(SECRET)]);
    let mut client = h.client();
    full_session(&mut client).await;
    assert_eq!(h.dials(), 1);
    assert_eq!(h.tokens_issued(), 0);
    assert_eq!(h.proxy.method_label().as_deref(), Some("password"));
}

#[tokio::test]
async fn secrets_project_supports_md5() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Md5(SECRET)]);
    let mut client = h.client();
    full_session(&mut client).await;
    assert_eq!(h.proxy.method_label().as_deref(), Some("password"));
}

#[tokio::test]
async fn wrong_secret_password_is_relayed_without_a_retry() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Scram("rotated-password")]);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, _) = expect_error(&mut client).await;
    assert_eq!(code, "28P01");
    assert_eq!(h.dials(), 1, "SCRAM failures are not retried with a token");
}

#[tokio::test]
async fn cleartext_fallback_retries_with_the_password_once_and_remembers_it() {
    let pki = test_pki();
    let h = Harness::new(
        &pki,
        auto(),
        vec![Rds::Pam(SECRET), Rds::Pam(SECRET), Rds::Pam(SECRET)],
    );
    let mut client = h.client();
    full_session(&mut client).await;
    let seen = h.seen().await;
    assert_eq!(seen.len(), 2, "token attempt, then password on a fresh connection");
    assert!(seen.iter().any(|s| s.cleartext.as_deref() == Some(TOKEN) && !s.signed_in));
    assert!(seen.iter().any(|s| s.cleartext.as_deref() == Some(SECRET) && s.signed_in));
    assert_eq!(h.proxy.method_label().as_deref(), Some("password"));

    // The next session goes straight to the password.
    let mut client = h.client();
    full_session(&mut client).await;
    let seen = h.seen().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].cleartext.as_deref(), Some(SECRET));
}

#[tokio::test]
async fn cached_method_is_re_probed_when_it_stops_working() {
    let pki = test_pki();
    // Learn "password" first, then the database switches the user to IAM.
    let h = Harness::new(
        &pki,
        auto(),
        vec![Rds::Pam(SECRET), Rds::Pam(SECRET), Rds::Pam(TOKEN), Rds::Pam(TOKEN)],
    );
    let mut client = h.client();
    full_session(&mut client).await;
    assert_eq!(h.proxy.method_label().as_deref(), Some("password"));
    h.seen().await;

    let mut client = h.client();
    full_session(&mut client).await;
    let seen = h.seen().await;
    assert_eq!(seen.len(), 2, "password refused, token on a fresh connection");
    assert_eq!(h.proxy.method_label().as_deref(), Some("IAM (auto)"));
}

#[tokio::test]
async fn both_credentials_refused_relays_the_last_error() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Pam("nobody"), Rds::Pam("nobody")]);
    let mut client = h.client();
    sign_in(&mut client, DB_USER, LOCAL_PASSWORD).await.unwrap();
    let (code, _) = expect_error(&mut client).await;
    assert_eq!(code, "28000");
    assert_eq!(h.dials(), 2, "exactly one retry");
}

#[tokio::test]
async fn cancel_request_is_forwarded_verbatim_over_tls() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Pam(TOKEN)]);
    let mut client = h.client();

    let mut cancel = 16u32.to_be_bytes().to_vec();
    cancel.extend_from_slice(&wire::CANCEL_REQUEST_CODE.to_be_bytes());
    cancel.extend_from_slice(&[0, 0, 0, 42, 1, 2, 3, 4]);
    send(&mut client, &cancel).await;

    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty(), "the client gets no reply to a cancel request");
    let seen = h.seen().await;
    assert_eq!(seen[0].cancel.as_deref(), Some(cancel.as_slice()));
    assert_eq!(h.tokens_issued(), 0, "cancel needs no credential");
}

#[tokio::test]
async fn concurrent_sessions_are_independent() {
    let pki = test_pki();
    let h = Harness::new(&pki, UpstreamAuth::IamOnly, vec![Rds::Pam(TOKEN); 5]);
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let mut client = h.client();
        tasks.push(tokio::spawn(async move { full_session(&mut client).await }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(h.seen().await.len(), 5);
}

#[tokio::test]
async fn probe_signs_in_and_logs_out() {
    let pki = test_pki();
    let h = Harness::new(&pki, auto(), vec![Rds::Pam(TOKEN)]);
    assert_eq!(h.proxy.probe("covered_db").await.as_deref(), Ok("IAM (auto)"));
    let seen = h.seen().await;
    assert!(seen[0].signed_in && seen[0].terminated);
    let startup = seen[0].startup.as_ref().unwrap();
    assert_eq!(startup.param("database"), Some(&b"covered_db"[..]));

    let h = Harness::new(&pki, auto(), vec![Rds::Scram("rotated")]);
    let err = h.proxy.probe("covered_db").await.unwrap_err();
    assert!(err.contains("password authentication failed"), "{}", err);
}

#[tokio::test]
async fn tunnel_pipes_carry_bytes_and_eof_both_ways() {
    let (dialer, inbox) = tunnel_pipes();
    let mut upstream = dialer.dial().await.unwrap();
    let mut pipe = inbox.lock().await.recv().await.unwrap();

    upstream.write_all(b"to rds").await.unwrap();
    let mut buf = [0u8; 6];
    pipe.reader.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"to rds");

    pipe.writer.write_all(b"to client").await.unwrap();
    let mut buf = [0u8; 9];
    upstream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"to client");

    // Tunnel side closes (remote FIN): the proxy sees EOF.
    drop(pipe.writer);
    let mut rest = Vec::new();
    upstream.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());

    // Proxy side closes: the tunnel sees EOF.
    drop(upstream);
    let mut rest = Vec::new();
    pipe.reader.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());

    drop(inbox);
    assert!(dialer.dial().await.is_err(), "dial fails once the tunnel is gone");
}

#[test]
fn md5_response_matches_postgres() {
    // Independent vectors (Python hashlib). The inner hash is what PostgreSQL
    // stores in pg_authid for an MD5 password ("md5" + md5(password + user)).
    assert_eq!(hex(&Md5::digest(b"postgrespostgres")), "3175bce1d3201d16594cebf9d7eb3f9d");
    assert_eq!(
        md5_response("user", "pw", &[0x01, 0x02, 0x03, 0x04]),
        "md546eacb7412409e1444bde22e56e70744"
    );
}

#[test]
fn embedded_rds_bundle_parses() {
    assert!(rds_tls_config().is_ok());
    let count = CertificateDer::pem_slice_iter(RDS_CA_BUNDLE).count();
    assert!(count > 100, "expected the full global bundle, got {} certificates", count);
}
