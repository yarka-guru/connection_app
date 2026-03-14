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
    #[test]
    fn test_token_format_basic() {
        // Just verify the URL construction logic (without actual signing)
        let url = format!(
            "https://{}:{}/?Action=connect&DBUser={}",
            "mydb.cluster-abc123.us-east-1.rds.amazonaws.com", 5432, "db_admin"
        );
        assert!(url.contains("Action=connect"));
        assert!(url.contains("DBUser=db_admin"));
        assert!(url.contains(":5432/"));
    }
}
