use crate::config::aws_config::parse_aws_config;

use aws_sdk_ec2 as ec2;
use aws_sdk_ecs as ecs;
use aws_sdk_rds as rds;
use aws_sdk_secretsmanager as secretsmanager;
use aws_sdk_ssm as ssm;
use aws_sdk_sts as sts;
use std::sync::OnceLock;

/// All AWS service clients for a given profile+region.
pub struct AwsClients {
    pub sts: sts::Client,
    pub ec2: ec2::Client,
    pub ecs: ecs::Client,
    pub rds: rds::Client,
    pub ssm: ssm::Client,
    pub secrets_manager: secretsmanager::Client,
}

/// SSO configuration extracted from an AWS profile.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SsoConfig {
    pub start_url: String,
    pub region: String,
    pub account_id: Option<String>,
    pub role_name: Option<String>,
    /// If using new-style `sso_session`, the session name.
    /// The AWS SDK caches tokens under SHA1(session_name) for sso-session profiles,
    /// vs SHA1(start_url) for legacy SSO profiles.
    pub session_name: Option<String>,
}

/// Authentication type detected from profile config (used by Phase 3 CLI).
#[derive(Debug)]
#[allow(dead_code)]
pub enum AuthType {
    Sso {
        sso_start_url: Option<String>,
        sso_session: Option<String>,
    },
    AssumeRoleMfa,
    AssumeRole,
    Process,
    Static,
    Unknown,
}

/// Cached custom CA cert bytes loaded from SSL_CERT_FILE env var.
static CUSTOM_CA_CERTS: OnceLock<Option<Vec<u8>>> = OnceLock::new();

/// Load custom CA certificates from SSL_CERT_FILE env var (if set).
pub fn load_custom_ca_certs() -> &'static Option<Vec<u8>> {
    CUSTOM_CA_CERTS.get_or_init(|| {
        let path = std::env::var("SSL_CERT_FILE").ok()?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                log::info!("Loaded custom CA certs from SSL_CERT_FILE={}", path);
                Some(bytes)
            }
            Err(e) => {
                log::warn!("Failed to read SSL_CERT_FILE={}: {}", path, e);
                None
            }
        }
    })
}

/// Build a TLS trust store with native OS root certificates and optional SSL_CERT_FILE certs.
/// This ensures corporate proxy CAs installed in the OS certificate store are trusted.
pub fn build_trust_store() -> aws_smithy_http_client::tls::TrustStore {
    use aws_smithy_http_client::tls;
    let mut trust_store = tls::TrustStore::default();
    if let Some(ca_bytes) = load_custom_ca_certs() {
        trust_store = trust_store.with_pem_certificate(ca_bytes.clone());
    }
    trust_store
}

/// Build AWS SDK config for a given profile and region.
/// Uses the aws-config crate which handles the full credential chain
/// including SSO, assume-role, process credentials, etc.
/// Always uses native OS root certificates + optional SSL_CERT_FILE certs.
pub async fn build_aws_config(profile: &str, region: &str) -> aws_config::SdkConfig {
    use aws_smithy_http_client::tls;
    let tls_context = tls::TlsContext::builder()
        .with_trust_store(build_trust_store())
        .build()
        .expect("valid TLS context");
    let http_client = aws_smithy_http_client::Builder::new()
        .tls_provider(tls::Provider::Rustls(tls::rustls_provider::CryptoMode::AwsLc))
        .tls_context(tls_context)
        .build_https();

    // Pin credentials to the explicitly selected profile. The default chain
    // checks environment variables BEFORE the profile, so an app launched
    // from a shell holding exported credentials (aws-vault exec, CI, etc.)
    // would silently connect EVERY profile to that account — mislabeling
    // environments. The profile provider still resolves the full profile
    // chain: SSO, assume-role, credential_process, and static keys.
    let profile_credentials = aws_config::profile::ProfileFileCredentialsProvider::builder()
        .profile_name(profile)
        .build();

    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .profile_name(profile)
        .credentials_provider(profile_credentials)
        .http_client(http_client)
        .load()
        .await
}

/// Create all AWS clients for a profile+region.
pub async fn create_aws_clients(profile: &str, region: &str) -> AwsClients {
    let config = build_aws_config(profile, region).await;

    AwsClients {
        sts: sts::Client::new(&config),
        ec2: ec2::Client::new(&config),
        ecs: ecs::Client::new(&config),
        rds: rds::Client::new(&config),
        ssm: ssm::Client::new(&config),
        secrets_manager: secretsmanager::Client::new(&config),
    }
}

/// Detect the authentication type for a profile (used by Phase 3 CLI).
#[allow(dead_code)]
pub async fn detect_auth_type(profile: &str) -> AuthType {
    let profiles = parse_aws_config().await;
    let config = match profiles.get(profile) {
        Some(c) => c,
        None => return AuthType::Unknown,
    };

    if config.contains_key("sso_start_url") || config.contains_key("sso_session") {
        return AuthType::Sso {
            sso_start_url: config.get("sso_start_url").cloned(),
            sso_session: config.get("sso_session").cloned(),
        };
    }

    if config.contains_key("role_arn") && config.contains_key("mfa_serial") {
        return AuthType::AssumeRoleMfa;
    }

    if config.contains_key("role_arn") {
        return AuthType::AssumeRole;
    }

    if config.contains_key("credential_process") {
        return AuthType::Process;
    }

    if config.contains_key("aws_access_key_id") {
        return AuthType::Static;
    }

    AuthType::Unknown
}

/// Extract SSO config for a profile. Handles both legacy SSO (keys on profile)
/// and new-style sso-session sections. Returns None for non-SSO profiles.
pub async fn get_sso_config(profile: &str) -> Option<SsoConfig> {
    let profiles = parse_aws_config().await;
    let config = profiles.get(profile)?;

    let mut start_url = config.get("sso_start_url").cloned();
    let mut region = config.get("sso_region").cloned();
    let account_id = config.get("sso_account_id").cloned();
    let role_name = config.get("sso_role_name").cloned();
    let mut session_name = None;

    // New-style: profile references an [sso-session <name>] section
    if let Some(sso_session) = config.get("sso_session") {
        session_name = Some(sso_session.clone());
        let session_key = format!("sso-session {}", sso_session);
        if let Some(session_config) = profiles.get(&session_key) {
            if start_url.is_none() {
                start_url = session_config.get("sso_start_url").cloned();
            }
            if region.is_none() {
                region = session_config.get("sso_region").cloned();
            }
        }
    }

    let start_url = start_url?;
    let region = region?;

    Some(SsoConfig {
        start_url,
        region,
        account_id,
        role_name,
        session_name,
    })
}

/// Check if any SSO-related config exists for a profile (used by Phase 3 CLI).
#[allow(dead_code)]
pub async fn is_sso_profile(profile: &str) -> bool {
    get_sso_config(profile).await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: credentials must come from the explicitly selected
    /// profile, never from inherited environment variables.
    ///
    /// The AWS SDK default chain checks env vars BEFORE the profile. An app
    /// launched from a shell holding exported credentials (`aws-vault exec`,
    /// `aws sso login` exports, CI) would silently connect EVERY profile to
    /// that account — e.g. a "prod" connection actually tunneling to staging
    /// (observed live 2026-06-10), or worse, the reverse.
    #[tokio::test]
    async fn profile_credentials_override_inherited_env_credentials() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("connapp-cred-pin-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config");
        let mut f = std::fs::File::create(&cfg_path).unwrap();
        writeln!(
            f,
            "[profile pin-test]\naws_access_key_id = PROFILEKEY123\naws_secret_access_key = profilesecret"
        )
        .unwrap();

        // SAFETY: test-only env mutation; no other test reads these vars.
        unsafe {
            std::env::set_var("AWS_CONFIG_FILE", &cfg_path);
            std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", dir.join("credentials"));
            std::env::set_var("AWS_ACCESS_KEY_ID", "ENVKEY999");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "envsecret");
            std::env::set_var("AWS_SESSION_TOKEN", "envtoken");
        }

        let config = build_aws_config("pin-test", "us-west-1").await;
        use aws_sdk_sts::config::ProvideCredentials;
        let creds = config
            .credentials_provider()
            .expect("credentials provider")
            .provide_credentials()
            .await
            .expect("credentials resolve");

        // SAFETY: see above.
        unsafe {
            std::env::remove_var("AWS_ACCESS_KEY_ID");
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
            std::env::remove_var("AWS_SESSION_TOKEN");
            std::env::remove_var("AWS_CONFIG_FILE");
            std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        }

        assert_eq!(
            creds.access_key_id(),
            "PROFILEKEY123",
            "inherited env credentials must not override the selected profile"
        );
    }
}

