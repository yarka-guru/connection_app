use crate::error::AppError;
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{
    sign, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4;
use std::time::{Duration, SystemTime};

/// RDS accepts IAM auth tokens for at most 15 minutes.
pub const MAX_TOKEN_LIFETIME: Duration = Duration::from_secs(900);

/// Test-only override of the token lifetime, in seconds (10..=900).
pub const TOKEN_LIFETIME_ENV: &str = "CONNECTION_APP_IAM_TOKEN_TTL_SECS";

/// Generate an RDS IAM authentication token.
///
/// The token is a presigned URL for `Action=connect` against the RDS endpoint,
/// signed with SigV4 using the `rds-db` service. The token is valid for 15 minutes.
///
/// This is equivalent to `aws rds generate-db-auth-token`.
pub async fn generate_rds_auth_token(
    config: &aws_config::SdkConfig,
    hostname: &str,
    port: u16,
    username: &str,
) -> Result<String, AppError> {
    generate_rds_auth_token_valid_for(config, hostname, port, username, MAX_TOKEN_LIFETIME).await
}

/// Generate an RDS IAM authentication token valid for `lifetime` (capped at 15 minutes).
pub async fn generate_rds_auth_token_valid_for(
    config: &aws_config::SdkConfig,
    hostname: &str,
    port: u16,
    username: &str,
    lifetime: Duration,
) -> Result<String, AppError> {
    // Resolve credentials from the SDK config
    let credentials_provider = config
        .credentials_provider()
        .ok_or_else(|| AppError::Aws("No credentials provider configured".to_string()))?;

    let credentials = credentials_provider
        .provide_credentials()
        .await
        .map_err(|e| AppError::Aws(format!("Failed to resolve AWS credentials: {}", e)))?;

    let region = config
        .region()
        .ok_or_else(|| AppError::Aws("No region configured".to_string()))?
        .to_string();

    // Build the presigned URL
    // The URL format is: https://{hostname}:{port}/?Action=connect&DBUser={username}
    let url = format!(
        "https://{}:{}/?Action=connect&DBUser={}",
        hostname, port, username
    );

    // Configure signing settings for presigned URL (query string signing)
    let mut signing_settings = SigningSettings::default();
    signing_settings.signature_location = SignatureLocation::QueryParams;
    signing_settings.expires_in = Some(lifetime.min(MAX_TOKEN_LIFETIME));

    let identity = credentials.into();
    let signing_params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&region)
        .name("rds-db")
        .time(SystemTime::now())
        .settings(signing_settings)
        .build()
        .map_err(|e| AppError::Aws(format!("Failed to build signing params: {}", e)))?;

    let signable_request =
        SignableRequest::new("GET", &url, std::iter::empty(), SignableBody::Bytes(&[]))
            .map_err(|e| AppError::Aws(format!("Failed to create signable request: {}", e)))?;

    let signing_params = signing_params.into();
    let (signing_instructions, _signature) = sign(signable_request, &signing_params)
        .map_err(|e| AppError::Aws(format!("Failed to sign RDS auth request: {}", e)))?
        .into_parts();

    // Apply signing instructions to build the final presigned URL
    let mut presigned_url = reqwest::Url::parse(&url)
        .map_err(|e| AppError::Aws(format!("Failed to parse URL: {}", e)))?;

    for (name, value) in signing_instructions.params() {
        presigned_url.query_pairs_mut().append_pair(name, value);
    }

    // The auth token is the presigned URL without the "https://" scheme prefix
    let token = presigned_url
        .as_str()
        .strip_prefix("https://")
        .unwrap_or(presigned_url.as_str())
        .to_string();

    Ok(token)
}

/// Token lifetime: 15 minutes, or the test override from `TOKEN_LIFETIME_ENV`.
pub fn token_lifetime() -> Duration {
    parse_token_lifetime(std::env::var(TOKEN_LIFETIME_ENV).ok().as_deref())
}

fn parse_token_lifetime(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs.clamp(10, MAX_TOKEN_LIFETIME.as_secs())))
        .unwrap_or(MAX_TOKEN_LIFETIME)
}

/// True while a token issued at `issued_at` is younger than `max_age`.
/// Uses wall-clock time: the token's expiry is wall-clock based, and a
/// monotonic clock stops while the machine sleeps.
fn is_fresh(issued_at: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(issued_at)
        .map(|age| age < max_age)
        .unwrap_or(false)
}

struct CachedToken {
    token: String,
    issued_at: SystemTime,
}

/// Caches one RDS IAM auth token and mints a new one once it is two thirds
/// through its lifetime (10 of 15 minutes). Every new token resolves AWS
/// credentials again.
pub struct RdsAuthTokenCache {
    config: aws_config::SdkConfig,
    hostname: String,
    port: u16,
    username: String,
    lifetime: Duration,
    refresh_after: Duration,
    cached: tokio::sync::Mutex<Option<CachedToken>>,
}

impl RdsAuthTokenCache {
    pub fn new(config: aws_config::SdkConfig, hostname: &str, port: u16, username: &str) -> Self {
        let lifetime = token_lifetime();
        Self {
            config,
            hostname: hostname.to_string(),
            port,
            username: username.to_string(),
            lifetime,
            refresh_after: lifetime * 2 / 3,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn token(&self) -> Result<String, AppError> {
        let mut cached = self.cached.lock().await;
        if let Some(c) = cached.as_ref()
            && is_fresh(c.issued_at, SystemTime::now(), self.refresh_after)
        {
            return Ok(c.token.clone());
        }
        let issued_at = SystemTime::now();
        let token = generate_rds_auth_token_valid_for(
            &self.config,
            &self.hostname,
            self.port,
            &self.username,
            self.lifetime,
        )
        .await?;
        log::info!(
            "Generated a new RDS IAM auth token for {}@{} (valid {}s, replaced after {}s)",
            self.username,
            self.hostname,
            self.lifetime.as_secs(),
            self.refresh_after.as_secs()
        );
        *cached = Some(CachedToken {
            token: token.clone(),
            issued_at,
        });
        Ok(token)
    }

    /// Drop the cached token so the next connection mints a fresh one.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        generate_rds_auth_token, generate_rds_auth_token_valid_for, is_fresh,
        parse_token_lifetime, RdsAuthTokenCache, MAX_TOKEN_LIFETIME,
    };
    use std::time::{Duration, SystemTime};
    use aws_credential_types::provider::SharedCredentialsProvider;
    use aws_credential_types::Credentials;

    fn sdk_config(session_token: Option<&str>) -> aws_config::SdkConfig {
        let credentials = Credentials::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token.map(str::to_string),
            None,
            "test",
        );
        aws_config::SdkConfig::builder()
            .region(aws_config::Region::new("us-west-1"))
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .build()
    }

    fn query_pairs(token: &str) -> Vec<(String, String)> {
        let url = reqwest::Url::parse(&format!("https://{}", token)).unwrap();
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[tokio::test]
    async fn test_token_is_presigned_rds_db_connect_url() {
        let token = generate_rds_auth_token(
            &sdk_config(Some("session/token+with=specials")),
            "covered-db-dev.abc123.us-west-1.rds.amazonaws.com",
            5433,
            "covered_app",
        )
        .await
        .unwrap();

        assert!(token.starts_with(
            "covered-db-dev.abc123.us-west-1.rds.amazonaws.com:5433/?Action=connect&DBUser=covered_app&"
        ));
        assert!(!token.contains("https://"));

        let pairs = query_pairs(&token);
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("X-Amz-Algorithm"), Some("AWS4-HMAC-SHA256"));
        assert_eq!(get("X-Amz-Expires"), Some("900"));
        assert_eq!(get("X-Amz-SignedHeaders"), Some("host"));
        assert_eq!(get("X-Amz-Security-Token"), Some("session/token+with=specials"));
        let credential = get("X-Amz-Credential").unwrap();
        assert!(credential.starts_with("AKIDEXAMPLE/"));
        assert!(credential.ends_with("/us-west-1/rds-db/aws4_request"));
        assert_eq!(get("X-Amz-Signature").map(str::len), Some(64));
    }

    #[tokio::test]
    async fn test_token_without_session_token_omits_it() {
        let token = generate_rds_auth_token(&sdk_config(None), "db.example.com", 5432, "db_admin")
            .await
            .unwrap();
        assert!(query_pairs(&token)
            .iter()
            .all(|(name, _)| name != "X-Amz-Security-Token"));
    }

    fn expires(token: &str) -> String {
        query_pairs(token)
            .into_iter()
            .find(|(k, _)| k == "X-Amz-Expires")
            .map(|(_, v)| v)
            .unwrap()
    }

    #[tokio::test]
    async fn test_token_lifetime_is_capped_at_15_minutes() {
        let config = sdk_config(None);
        let short = generate_rds_auth_token_valid_for(&config, "db.example.com", 5432, "u", Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(expires(&short), "30");
        let long = generate_rds_auth_token_valid_for(&config, "db.example.com", 5432, "u", Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(expires(&long), "900");
    }

    #[test]
    fn test_lifetime_override_parsing() {
        assert_eq!(parse_token_lifetime(None), MAX_TOKEN_LIFETIME);
        assert_eq!(parse_token_lifetime(Some("30")), Duration::from_secs(30));
        assert_eq!(parse_token_lifetime(Some(" 45 ")), Duration::from_secs(45));
        assert_eq!(parse_token_lifetime(Some("1")), Duration::from_secs(10));
        assert_eq!(parse_token_lifetime(Some("100000")), MAX_TOKEN_LIFETIME);
        assert_eq!(parse_token_lifetime(Some("soon")), MAX_TOKEN_LIFETIME);
    }

    #[test]
    fn test_freshness_uses_age_and_rejects_clock_going_backwards() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let max_age = Duration::from_secs(600);
        assert!(is_fresh(t0, t0, max_age));
        assert!(is_fresh(t0, t0 + Duration::from_secs(599), max_age));
        assert!(!is_fresh(t0, t0 + Duration::from_secs(600), max_age));
        // A wall clock that jumped back must not keep a token alive.
        assert!(!is_fresh(t0, t0 - Duration::from_secs(5), max_age));
    }

    #[tokio::test]
    async fn test_cache_reuses_token_until_refresh_age_and_after_invalidate() {
        let cache = RdsAuthTokenCache::new(sdk_config(None), "db.example.com", 5432, "u");
        let first = cache.token().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(cache.token().await.unwrap(), first, "token must be reused while fresh");
        cache.invalidate().await;
        let second = cache.token().await.unwrap();
        assert_ne!(second, first, "a new signing time must produce a new token");

        // Age the cached token past the refresh point.
        {
            let mut cached = cache.cached.lock().await;
            cached.as_mut().unwrap().issued_at -= cache.refresh_after;
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_ne!(cache.token().await.unwrap(), second, "stale token must be replaced");
    }
}
