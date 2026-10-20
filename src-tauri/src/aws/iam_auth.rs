use crate::error::AppError;
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{
    sign, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4;
use std::time::{Duration, SystemTime};

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
    signing_settings.expires_in = Some(Duration::from_secs(900)); // 15 minutes

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

#[cfg(test)]
mod tests {
    use super::generate_rds_auth_token;
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
}
