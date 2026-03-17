use crate::aws::credentials::get_sso_config;
use crate::error::AppError;
use aws_sdk_ssooidc as ssooidc;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Format an AWS SDK error with the full error chain for diagnostics.
fn format_sdk_error(context: &str, err: &dyn std::error::Error) -> String {
    let mut msg = format!("{}: {}", context, err);
    let mut source = err.source();
    while let Some(cause) = source {
        msg.push_str(&format!(" → {}", cause));
        source = cause.source();
    }
    msg
}

const CLIENT_NAME: &str = "rds-connect-app";
const CLIENT_TYPE: &str = "public";
const TOKEN_EXPIRY_BUFFER_MS: i64 = 5 * 60 * 1000; // 5 minutes
const POLL_TIMEOUT_MS: u64 = 10 * 60 * 1000; // 10 minutes max

/// Cached OIDC client registration (valid ~90 days, keyed by region).
static CLIENT_REGISTRATION_CACHE: Mutex<Option<HashMap<String, ClientRegistration>>> =
    Mutex::new(None);

#[derive(Clone)]
struct ClientRegistration {
    client_id: String,
    client_secret: String,
    client_secret_expires_at: i64,
}

/// Trait for SSO event callbacks. Allows decoupling from Tauri.
pub trait SsoEventHandler: Send + Sync {
    fn on_status(&self, message: &str, connection_id: Option<&str>);
    fn on_open_url(&self, url: &str, connection_id: Option<&str>);
}

/// GUI event handler (Tauri).
#[cfg(feature = "gui")]
pub struct TauriSsoHandler {
    pub app_handle: tauri::AppHandle,
}

#[cfg(feature = "gui")]
impl SsoEventHandler for TauriSsoHandler {
    fn on_status(&self, message: &str, connection_id: Option<&str>) {
        use tauri::Emitter;
        let mut payload = serde_json::json!({ "message": message });
        if let Some(id) = connection_id {
            payload["connectionId"] = serde_json::json!(id);
        }
        let _ = self.app_handle.emit("sso-status", &payload);
    }

    fn on_open_url(&self, url: &str, connection_id: Option<&str>) {
        use tauri::Emitter;
        let mut payload = serde_json::json!({ "url": url });
        if let Some(id) = connection_id {
            payload["connectionId"] = serde_json::json!(id);
        }
        let _ = self.app_handle.emit("sso-open-url", &payload);

        // Also open the URL in the default browser
        use tauri_plugin_opener::OpenerExt;
        let _ = self.app_handle.opener().open_url(url, None::<&str>);
    }
}

/// CLI event handler (prints to stdout, opens browser via system command).
pub struct CliSsoHandler;

impl SsoEventHandler for CliSsoHandler {
    fn on_status(&self, message: &str, _connection_id: Option<&str>) {
        eprintln!("  \u{23F3} {}", message);
    }

    fn on_open_url(&self, url: &str, _connection_id: Option<&str>) {
        eprintln!(
            "\n  \u{1F310} Open this URL in your browser to authorize:\n     {}\n",
            url
        );
        // Only open HTTPS URLs to prevent protocol abuse (file://, javascript:, etc.)
        if !url.starts_with("https://") {
            eprintln!("  Warning: refusing to open non-HTTPS URL");
            return;
        }
        // Try to open browser automatically
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", url])
                .spawn();
        }
    }
}

/// Compute the cache filepath for an SSO token.
/// AWS CLI uses SHA1 of the startUrl.
fn get_sso_token_filepath(key: &str) -> PathBuf {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    crate::config::aws_config::get_aws_dir()
        .join("sso")
        .join("cache")
        .join(format!("{}.json", hash))
}

/// Read and parse a cached SSO token. Returns None if missing or malformed.
async fn read_sso_token(key: &str) -> Option<serde_json::Value> {
    let filepath = get_sso_token_filepath(key);
    let content = tokio::fs::read_to_string(&filepath).await.ok()?;
    let token: serde_json::Value = serde_json::from_str(&content).ok()?;

    if token.get("accessToken").is_none() || token.get("expiresAt").is_none() {
        return None;
    }

    Some(token)
}

/// Check if a cached SSO token is still valid (with buffer).
fn is_sso_token_valid(token: &serde_json::Value) -> bool {
    let expires_at = match token.get("expiresAt").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return false,
    };

    let expires_at_ms = match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(dt) => dt.timestamp_millis(),
        Err(_) => return false,
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    expires_at_ms > now_ms + TOKEN_EXPIRY_BUFFER_MS
}

/// Write an SSO token to the AWS CLI-compatible cache location.
async fn write_sso_token(key: &str, token_data: &serde_json::Value) -> Result<(), AppError> {
    let filepath = get_sso_token_filepath(key);
    let dir = filepath
        .parent()
        .ok_or_else(|| AppError::Sso("SSO cache filepath has no parent directory".to_string()))?;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| AppError::Sso(format!("Failed to create SSO cache directory: {}", e)))?;

    let json = serde_json::to_string_pretty(token_data)?;
    tokio::fs::write(&filepath, &json)
        .await
        .map_err(|e| AppError::Sso(format!("Failed to write SSO token: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&filepath, perms)
            .await
            .map_err(|e| AppError::Sso(format!("Failed to set token permissions: {}", e)))?;
    }

    Ok(())
}

/// Register an OIDC client with AWS SSO, or return cached registration.
async fn register_client(
    client: &ssooidc::Client,
    sso_region: &str,
) -> Result<(String, String), AppError> {
    {
        let cache = CLIENT_REGISTRATION_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(ref map) = *cache
            && let Some(reg) = map.get(sso_region)
        {
            let now_secs = chrono::Utc::now().timestamp();
            if reg.client_secret_expires_at > now_secs {
                return Ok((reg.client_id.clone(), reg.client_secret.clone()));
            }
        }
    }

    let response = client
        .register_client()
        .client_name(CLIENT_NAME)
        .client_type(CLIENT_TYPE)
        .send()
        .await
        .map_err(|e| AppError::Sso(format_sdk_error("Failed to register OIDC client", &e)))?;

    let client_id = response
        .client_id()
        .ok_or_else(|| AppError::Sso("No client_id in registration response".to_string()))?
        .to_string();
    let client_secret = response
        .client_secret()
        .ok_or_else(|| AppError::Sso("No client_secret in registration response".to_string()))?
        .to_string();
    let expires_at = response.client_secret_expires_at();

    {
        let mut cache = CLIENT_REGISTRATION_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let map = cache.get_or_insert_with(HashMap::new);
        map.insert(
            sso_region.to_string(),
            ClientRegistration {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                client_secret_expires_at: expires_at,
            },
        );
    }

    Ok((client_id, client_secret))
}

/// Start the device authorization flow.
async fn start_device_authorization(
    client: &ssooidc::Client,
    client_id: &str,
    client_secret: &str,
    start_url: &str,
) -> Result<DeviceAuth, AppError> {
    let response = client
        .start_device_authorization()
        .client_id(client_id)
        .client_secret(client_secret)
        .start_url(start_url)
        .send()
        .await
        .map_err(|e| AppError::Sso(format_sdk_error("Failed to start device authorization", &e)))?;

    Ok(DeviceAuth {
        device_code: response
            .device_code()
            .ok_or_else(|| AppError::Sso("No device_code".to_string()))?
            .to_string(),
        verification_uri_complete: response
            .verification_uri_complete()
            .map(|s| s.to_string()),
        verification_uri: response.verification_uri().map(|s| s.to_string()),
        expires_in: response.expires_in(),
        interval: response.interval(),
    })
}

struct DeviceAuth {
    device_code: String,
    verification_uri_complete: Option<String>,
    verification_uri: Option<String>,
    expires_in: i32,
    interval: i32,
}

/// Poll for token after user authorizes in browser.
#[allow(clippy::too_many_arguments)]
async fn poll_for_token(
    client: &ssooidc::Client,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
    interval: i32,
    expires_in: i32,
    handler: &dyn SsoEventHandler,
    connection_id: Option<&str>,
) -> Result<serde_json::Value, AppError> {
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(
            std::cmp::min(expires_in as u64 * 1000, POLL_TIMEOUT_MS),
        );
    let mut poll_interval = std::time::Duration::from_secs(std::cmp::max(interval, 5) as u64);

    while std::time::Instant::now() < deadline {
        tokio::time::sleep(poll_interval).await;

        match client
            .create_token()
            .client_id(client_id)
            .client_secret(client_secret)
            .grant_type("urn:ietf:params:oauth:grant-type:device_code")
            .device_code(device_code)
            .send()
            .await
        {
            Ok(response) => {
                let access_token = response
                    .access_token()
                    .ok_or_else(|| AppError::Sso("No access_token in response".to_string()))?;
                let expires_in_secs = response.expires_in();
                let expires_at =
                    chrono::Utc::now() + chrono::Duration::seconds(expires_in_secs as i64);

                let mut token = serde_json::json!({
                    "accessToken": access_token,
                    "expiresAt": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                });

                // Include refreshToken if available — needed by the AWS SDK
                // for automatic token refresh on sso-session profiles.
                if let Some(rt) = response.refresh_token() {
                    token["refreshToken"] = serde_json::json!(rt);
                }

                return Ok(token);
            }
            Err(sdk_err) => {
                if let aws_sdk_ssooidc::error::SdkError::ServiceError(ref service_err) =
                    sdk_err
                {
                    let err = service_err.err();
                    if err.is_authorization_pending_exception() {
                        handler.on_status(
                            "Waiting for authorization in browser...",
                            connection_id,
                        );
                        continue;
                    }
                    if err.is_slow_down_exception() {
                        poll_interval += std::time::Duration::from_secs(5);
                        continue;
                    }
                    if err.is_expired_token_exception() {
                        return Err(AppError::Sso(
                            "SSO authorization expired. Please try connecting again.".to_string(),
                        ));
                    }
                }

                return Err(AppError::Sso(format!("SSO token polling failed: {}", sdk_err)));
            }
        }
    }

    Err(AppError::Sso(
        "SSO authorization timed out. Please try connecting again.".to_string(),
    ))
}

/// Orchestrate the full SSO login flow:
/// register client → start device auth → open browser → poll for token → cache
pub async fn perform_sso_login(
    sso_start_url: &str,
    sso_region: &str,
    sso_session_name: Option<&str>,
    handler: &dyn SsoEventHandler,
    connection_id: Option<&str>,
) -> Result<(), AppError> {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(sso_region.to_string()))
        .no_credentials()
        .load()
        .await;
    let sso_oidc_client = ssooidc::Client::new(&config);

    handler.on_status("Registering SSO client...", connection_id);
    let (client_id, client_secret) = register_client(&sso_oidc_client, sso_region).await?;

    handler.on_status("Starting device authorization...", connection_id);
    let device_auth = start_device_authorization(
        &sso_oidc_client,
        &client_id,
        &client_secret,
        sso_start_url,
    )
    .await?;

    // Signal to open browser — only allow HTTPS URLs
    let url_to_open = device_auth
        .verification_uri_complete
        .as_deref()
        .or(device_auth.verification_uri.as_deref());

    match url_to_open {
        Some(url) if url.starts_with("https://") => {
            handler.on_status(
                "Waiting for SSO authorization in browser...",
                connection_id,
            );
            handler.on_open_url(url, connection_id);
        }
        Some(url) => {
            return Err(AppError::Sso(format!(
                "SSO returned an invalid verification URL: {}",
                url
            )));
        }
        None => {
            return Err(AppError::Sso(
                "SSO returned an empty verification URL".to_string(),
            ));
        }
    }

    // Poll until user authorizes
    let mut token_data = poll_for_token(
        &sso_oidc_client,
        &client_id,
        &client_secret,
        &device_auth.device_code,
        device_auth.interval,
        device_auth.expires_in,
        handler,
        connection_id,
    )
    .await?;

    token_data["startUrl"] = serde_json::json!(sso_start_url);
    token_data["region"] = serde_json::json!(sso_region);

    // Include OIDC registration fields — the AWS SDK needs these for
    // automatic token refresh on sso-session profiles.
    token_data["clientId"] = serde_json::json!(client_id);
    token_data["clientSecret"] = serde_json::json!(client_secret);
    {
        let cache = CLIENT_REGISTRATION_CACHE
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(ref map) = *cache
            && let Some(reg) = map.get(sso_region)
        {
            token_data["registrationExpiresAt"] = serde_json::json!(
                chrono::DateTime::from_timestamp(reg.client_secret_expires_at, 0)
                    .unwrap_or_default()
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            );
        }
    }

    // Write token to the cache location the AWS SDK expects:
    // - sso-session profiles: SHA1(session_name)
    // - legacy SSO profiles: SHA1(start_url)
    let cache_key = sso_session_name.unwrap_or(sso_start_url);
    write_sso_token(cache_key, &token_data).await?;

    // For sso-session profiles, also write under SHA1(start_url) for CLI compatibility
    if sso_session_name.is_some() {
        write_sso_token(sso_start_url, &token_data).await?;
    }

    handler.on_status("SSO login successful", connection_id);
    Ok(())
}

/// High-level entry: check if profile needs SSO login, perform if needed.
pub async fn ensure_sso_session(
    profile: &str,
    handler: &dyn SsoEventHandler,
    connection_id: Option<&str>,
) -> Result<(), AppError> {
    let sso_config = match get_sso_config(profile).await {
        Some(c) => c,
        None => return Ok(()), // Not an SSO profile
    };

    // The AWS SDK caches tokens under SHA1(session_name) for sso-session profiles,
    // and SHA1(start_url) for legacy SSO profiles. Use the matching key.
    let cache_key = sso_config
        .session_name
        .as_deref()
        .unwrap_or(&sso_config.start_url);

    // Check cached token
    if let Some(cached_token) = read_sso_token(cache_key).await
        && is_sso_token_valid(&cached_token)
    {
        handler.on_status("SSO session valid", connection_id);
        return Ok(());
    }

    // Token expired or missing — perform login
    handler.on_status("SSO session expired, starting login...", connection_id);
    perform_sso_login(
        &sso_config.start_url,
        &sso_config.region,
        sso_config.session_name.as_deref(),
        handler,
        connection_id,
    )
    .await
}
