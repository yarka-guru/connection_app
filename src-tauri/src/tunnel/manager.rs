use crate::aws::credentials::{build_aws_config, create_aws_clients, AwsClients};
use crate::aws::iam_auth;
use crate::aws::operations;
use crate::aws::sso::{ensure_sso_session, TauriSsoHandler};
use crate::config::accounts;
use crate::config::aws_config::read_aws_config;
use crate::config::environments::{
    account_to_verify, effective_project_config, find_environment, pick_profile, resolve_environments,
    ResolvedEnvironment,
};
use crate::config::local_passwords;
use crate::config::preferences;
use crate::config::projects::{
    get_default_port_for_engine, load_project_configs, AuthMode, ProjectConfig,
};
use crate::config::validation::is_shell_safe;
use crate::error::AppError;
use crate::history::{self, HistoryEntry};
use crate::tunnel::native::{self, LocalSource, PipeInbox};
use crate::tunnel::pg_proxy::{self, PgAuthProxy, ProxySettings, SecretRefetch, UpstreamAuth};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// Retry configuration constants (matching connect.js)
const PORT_FORWARDING_MAX_RETRIES: u32 = 2;
const AUTO_RECONNECT_MAX_RETRIES: u32 = 3;
const AUTO_RECONNECT_DELAY_MS: u64 = 3000;

// Health check constants
const HEALTH_CHECK_INTERVAL_SECS: u64 = 30;

// Sign-in check through the auth proxy right after the tunnel is up
const AUTH_PROBE_TIMEOUT_SECS: u64 = 30;

// Validation patterns
static PROFILE_SAFE_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._-]+$").unwrap());
static INSTANCE_ID_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^i-[a-f0-9]{8,17}$").unwrap());
static HOSTNAME_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9.-]+$").unwrap());
static IP_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^(\d{1,3}\.){3}\d{1,3}$").unwrap());

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConnectionInfo {
    pub host: String,
    pub port: String,
    #[serde(default)]
    pub environment: String,
    #[serde(rename = "connectionType", default)]
    pub connection_type: String,
    // RDS-specific fields (None for service connections)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(rename = "rdsEndpoint", skip_serializing_if = "Option::is_none")]
    pub rds_endpoint: Option<String>,
    #[serde(rename = "instanceId", skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    // Service-specific fields
    #[serde(rename = "serviceType", skip_serializing_if = "Option::is_none")]
    pub service_type: Option<String>,
    #[serde(rename = "remoteHost", skip_serializing_if = "Option::is_none")]
    pub remote_host: Option<String>,
    #[serde(rename = "targetType", skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    // SSH-specific: pre-built SSH command for copy-paste
    #[serde(rename = "sshCommand", skip_serializing_if = "Option::is_none")]
    pub ssh_command: Option<String>,
    // Auth proxy in use: upstream method "IAM", "IAM (auto)" or "password".
    // `password` is then the stable local password.
    #[serde(rename = "authMethod", default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub id: String,
    pub project_key: String,
    pub environment: String,
    pub profile: String,
    pub local_port: String,
    pub connection_info: ConnectionInfo,
    cancel_token: CancellationToken,
}

/// Sign-in check through the auth proxy, run once the tunnel is up. Kept
/// out of `connect` so callers can run it without holding the manager lock.
pub struct AuthCheck {
    proxy: Arc<PgAuthProxy>,
    database: String,
    connection_id: String,
    connections: Arc<Mutex<HashMap<String, Connection>>>,
    app_handle: AppHandle,
}

impl AuthCheck {
    /// Sign in once and log out; records and returns the auth method label.
    /// `None` when the connection was disconnected meanwhile.
    pub async fn run(self) -> Option<String> {
        emit_status_event(&self.app_handle, "Checking database sign-in...", Some(&self.connection_id));
        let probe = tokio::time::timeout(
            tokio::time::Duration::from_secs(AUTH_PROBE_TIMEOUT_SECS),
            self.proxy.probe(&self.database),
        )
        .await;
        let label = match probe {
            Ok(Ok(method)) => method,
            Ok(Err(e)) => {
                log::warn!("Database sign-in check failed: {}", e);
                format!("check failed: {}", e)
            }
            Err(_) => {
                log::warn!("Database sign-in check timed out");
                "check timed out".to_string()
            }
        };
        let mut guard = self.connections.lock().await;
        record_auth_method(&mut guard, &self.connection_id, label)
    }
}

/// Store the auth method on a connection; `None` if it no longer exists.
fn record_auth_method(
    connections: &mut HashMap<String, Connection>,
    connection_id: &str,
    label: String,
) -> Option<String> {
    let conn = connections.get_mut(connection_id)?;
    conn.connection_info.auth_method = Some(label.clone());
    Some(label)
}

/// Manages all active tunnel connections.
pub struct TunnelManager {
    connections: Arc<Mutex<HashMap<String, Connection>>>,
    app_handle: AppHandle,
}

/// Attempt to identify which process is holding a given port.
/// Returns a string like `"postgres (PID 12345)"` or `None` if detection fails.
/// Uses synchronous `std::process::Command` — safe to call from sync context.
fn get_port_holder(port: u16) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("lsof")
            .args([
                "-i", &format!(":{}", port),
                "-sTCP:LISTEN",
                "-P", "-n",
            ])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Skip header line, parse first data line: COMMAND PID USER ...
        let line = stdout.lines().nth(1)?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            Some(format!("{} (PID {})", parts[0], parts[1]))
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("ss")
            .args(["-tlnp", "sport", &format!("= :{}", port)])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Look for users:(("process",pid=12345,...)) in any line
        for line in stdout.lines() {
            if let Some(users_start) = line.find("users:((") {
                let rest = &line[users_start..];
                // Extract process name from ("name",pid=N,...)
                let name = rest
                    .split('"')
                    .nth(1)
                    .unwrap_or("unknown");
                let pid = rest
                    .split("pid=")
                    .nth(1)
                    .and_then(|s| s.split([',', ')']).next())
                    .unwrap_or("?");
                return Some(format!("{} (PID {})", name, pid));
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("netstat")
            .args(["-aon"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let port_str = format!(":{}", port);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // TCP  0.0.0.0:PORT  0.0.0.0:0  LISTENING  PID
            if parts.len() >= 5
                && parts[0] == "TCP"
                && parts[1].ends_with(&port_str)
                && parts[3] == "LISTENING"
            {
                let pid = parts[4];
                // Try to get process name via tasklist
                if let Ok(task_output) = std::process::Command::new("tasklist")
                    .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
                    .output()
                {
                    let task_stdout = String::from_utf8_lossy(&task_output.stdout);
                    if let Some(name) = task_stdout.split('"').nth(1) {
                        return Some(format!("{} (PID {})", name, pid));
                    }
                }
                return Some(format!("PID {}", pid));
            }
        }
        None
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

impl TunnelManager {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            app_handle,
        }
    }

    /// Check if a port is available by attempting to bind with SO_REUSEADDR.
    /// Using SO_REUSEADDR is critical on Linux where TIME_WAIT sockets from
    /// previous connections can block a plain bind for up to 60 seconds.
    fn is_port_available(port: u16) -> bool {
        let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
        let socket = match socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        ) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let _ = socket.set_reuse_address(true);
        socket.bind(&addr.into()).is_ok()
    }

    /// Get all ports currently in use by active connections.
    pub async fn get_used_ports(&self) -> Vec<String> {
        let guard = self.connections.lock().await;
        guard.values().map(|c| c.local_port.clone()).collect()
    }

    /// Get all active connections.
    pub async fn get_active_connections(&self) -> Vec<ActiveConnectionInfo> {
        let guard = self.connections.lock().await;
        guard
            .values()
            .map(|c| ActiveConnectionInfo {
                id: c.id.clone(),
                project_key: c.project_key.clone(),
                environment: c.environment.clone(),
                profile: c.profile.clone(),
                local_port: c.local_port.clone(),
                connection_info: c.connection_info.clone(),
                status: "connected".to_string(),
            })
            .collect()
    }

    /// Connect to an environment of a project (dispatches to RDS or service
    /// based on connectionType). The profile is the environment's resolved
    /// profile unless `profile_override` names another one for its account.
    /// For auth-proxy connections also returns the sign-in check to run next.
    pub async fn connect(
        &self,
        project_key: &str,
        environment: &str,
        profile_override: Option<&str>,
        local_port: Option<&str>,
        database: Option<&str>,
        used_ports: &[String],
    ) -> Result<(String, ConnectionInfo, Option<AuthCheck>), AppError> {
        if !is_shell_safe(environment) {
            return Err(AppError::General(format!(
                "Invalid environment name: {}",
                environment
            )));
        }

        // Load project config
        let configs = load_project_configs().await?;
        let project = configs.get(project_key).ok_or_else(|| {
            AppError::Config(format!("Unknown project: {}", project_key))
        })?;

        // Resolve the environment to a profile
        let profiles = read_aws_config().await?;
        let profile_accounts =
            accounts::profile_accounts(&profiles, &accounts::load_account_cache().await);
        let prefs = preferences::load_preferences().await;
        let resolved_all = resolve_environments(project, &configs, &profile_accounts, &prefs);
        let resolved = find_environment(&resolved_all, environment)
            .cloned()
            .ok_or_else(|| {
                AppError::Config(format!(
                    "Unknown environment '{}' in project {}",
                    environment, project_key
                ))
            })?;
        let profile = pick_profile(&resolved, profile_override)?;
        let profile = profile.as_str();

        // Validate profile
        if !PROFILE_SAFE_PATTERN.is_match(profile) {
            return Err(AppError::General(format!(
                "Invalid profile name: {}",
                profile
            )));
        }

        let project_config = &effective_project_config(project, &resolved);

        // Determine port
        let requested_port = local_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| resolved.environment.port.clone());

        let mut port_num: u16 = requested_port
            .parse()
            .map_err(|_| AppError::General(format!("Invalid port number: {}", requested_port)))?;

        // Collect ports already used by our active connections
        let all_used_ports: std::collections::HashSet<u16> = {
            let guard = self.connections.lock().await;
            let active_ports: Vec<u16> = guard
                .values()
                .filter_map(|c| c.local_port.parse().ok())
                .collect();

            used_ports
                .iter()
                .filter_map(|p| p.parse().ok())
                .chain(active_ports)
                .collect()
        };

        // If our own app holds the port, auto-increment to find the next free one.
        // If an external process holds it, report the error so the user can decide.
        let user_specified_port = local_port.is_some();
        if all_used_ports.contains(&port_num) || !Self::is_port_available(port_num) {
            if user_specified_port {
                // User explicitly requested this port — don't auto-increment
                let holder = get_port_holder(port_num)
                    .unwrap_or_else(|| "another process".to_string());
                return Err(AppError::Tunnel(format!(
                    "Port {} is already in use by {}. Close it or change the port in project settings.",
                    port_num, holder
                )));
            }

            // Auto-increment: try up to 100 ports above the requested one
            let mut found = false;
            for offset in 1..=100u16 {
                let candidate = port_num.saturating_add(offset);
                if candidate > 65534 {
                    break;
                }
                if !all_used_ports.contains(&candidate) && Self::is_port_available(candidate) {
                    log::info!(
                        "Port {} in use, auto-assigned port {} instead",
                        port_num, candidate
                    );
                    port_num = candidate;
                    found = true;
                    break;
                }
            }
            if !found {
                let holder = get_port_holder(port_num)
                    .unwrap_or_else(|| "another process".to_string());
                return Err(AppError::Tunnel(format!(
                    "Port {} is already in use by {}. No free ports found nearby.",
                    requested_port, holder
                )));
            }
        }

        let port_to_use = port_num.to_string();

        // Generate connection ID
        let connection_id = format!("conn_{}", &uuid::Uuid::new_v4().to_string()[..8]);

        // SSO pre-flight
        let sso_handler = TauriSsoHandler {
            app_handle: self.app_handle.clone(),
        };
        ensure_sso_session(profile, &sso_handler, Some(&connection_id)).await?;

        // Create AWS clients
        let clients = create_aws_clients(profile, &project_config.region).await;

        // The profile must sign in to the environment's account
        if let Some(expected) = account_to_verify(&resolved) {
            self.emit_status("Checking AWS account...", Some(&connection_id));
            let actual = operations::caller_account_id(&clients).await?;
            operations::ensure_account(expected, &actual, &resolved.environment.name)?;
        }

        // Dispatch based on connection type
        let (connection_info, tunnel_target, proxy_settings) = match project_config.connection_type.as_str() {
            "service" => {
                let (info, target) = self.resolve_service_target(&clients, &connection_id, project_key, &resolved, project_config, &port_to_use).await?;
                (info, target, None)
            }
            _ => self.resolve_rds_target(&clients, &connection_id, project_key, &resolved, profile, project_config, &port_to_use, database).await?,
        };

        let cancel_token = CancellationToken::new();
        // Stops the auth proxy listener on disconnect or when the tunnel gives up.
        let proxy_cancel = cancel_token.child_token();

        // The auth proxy owns the local port for the whole connection, across
        // tunnel reconnects; each tunnel session takes its connections as pipes.
        let mut auth_proxy: Option<Arc<PgAuthProxy>> = None;
        let mut pipe_inbox: Option<PipeInbox> = None;
        if let Some(settings) = proxy_settings {
            let TunnelTarget::RemoteHost { remote_host, remote_port, .. } = &tunnel_target else {
                return Err(AppError::Tunnel("Auth proxy needs an RDS target".to_string()));
            };
            let rds_port: u16 = remote_port
                .parse()
                .map_err(|_| AppError::General(format!("Invalid RDS port number: {}", remote_port)))?;
            let (listener_v4, listener_v6) =
                native::bind_local_listeners(port_num).map_err(AppError::Tunnel)?;
            let (dialer, inbox) = pg_proxy::tunnel_pipes();
            let proxy = Arc::new(
                PgAuthProxy::for_rds(settings, remote_host, rds_port, Arc::new(dialer))
                    .map_err(AppError::Tunnel)?,
            );
            tokio::spawn(pg_proxy::run_listener(
                proxy.clone(),
                listener_v4,
                listener_v6,
                proxy_cancel.clone(),
            ));
            auth_proxy = Some(proxy);
            pipe_inbox = Some(inbox);
        }

        let connection = Connection {
            id: connection_id.clone(),
            project_key: project_key.to_string(),
            environment: resolved.environment.name.clone(),
            profile: profile.to_string(),
            local_port: port_to_use.clone(),
            connection_info: connection_info.clone(),
            cancel_token: cancel_token.clone(),
        };

        // Store connection
        {
            let mut guard = self.connections.lock().await;
            guard.insert(connection_id.clone(), connection);
        }

        // Channel to signal when the tunnel is actually ready
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

        // Clone values needed for health check (before they're moved into spawn)
        let health_port = port_to_use.clone();
        let health_cancel = cancel_token.clone();

        // Spawn background task for port forwarding lifecycle
        let app_handle = self.app_handle.clone();
        let connections = self.connections.clone();
        let conn_id = connection_id.clone();
        let project_key_owned = project_key.to_string();
        let profile_owned = profile.to_string();
        let conn_label = format!("{} {}", project_key, resolved.environment.name);
        // Auto-reconnect rediscovers with this same effective config and clients,
        // so it never switches environment overrides or profile mid-session.
        let project_config = project_config.clone();
        let lifecycle_proxy_cancel = proxy_cancel.clone();

        tokio::spawn(async move {
            let result = run_tunnel_lifecycle(
                &app_handle,
                &clients,
                &conn_id,
                &port_to_use,
                &project_config,
                tunnel_target,
                pipe_inbox,
                cancel_token.clone(),
                Some(ready_tx),
                &conn_label,
                &project_key_owned,
                &profile_owned,
            )
            .await;

            // Clean up connection
            {
                let mut guard = connections.lock().await;
                guard.remove(&conn_id);
            }

            match result {
                Ok(()) => {
                    history::log_event(HistoryEntry {
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        event_type: "disconnected".to_string(),
                        connection_id: conn_id.clone(),
                        project_key: project_key_owned.clone(),
                        profile: profile_owned.clone(),
                        details: Some("session_ended".to_string()),
                    })
                    .await;
                    // Only notify if this was not a user-initiated disconnect
                    if !cancel_token.is_cancelled() {
                        send_notification(
                            &app_handle,
                            "Connection Lost",
                            &format!("{} disconnected", conn_label),
                        );
                    }
                    let _ = app_handle.emit(
                        "disconnected",
                        serde_json::json!({
                            "connectionId": conn_id,
                            "reason": "session_ended"
                        }),
                    );
                }
                Err(e) => {
                    history::log_event(HistoryEntry {
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        event_type: "error".to_string(),
                        connection_id: conn_id.clone(),
                        project_key: project_key_owned.clone(),
                        profile: profile_owned.clone(),
                        details: Some(e.to_string()),
                    })
                    .await;
                    send_notification(
                        &app_handle,
                        "Connection Failed",
                        &format!("{}: {}", conn_label, e),
                    );
                    let _ = app_handle.emit(
                        "connection-error",
                        serde_json::json!({
                            "connectionId": conn_id,
                            "message": e.to_string()
                        }),
                    );
                    let _ = app_handle.emit(
                        "disconnected",
                        serde_json::json!({
                            "connectionId": conn_id,
                            "reason": "error"
                        }),
                    );
                }
            }
            lifecycle_proxy_cancel.cancel();
        });

        self.emit_status("Starting port forwarding...", Some(&connection_id));

        // Wait for the tunnel to actually be ready (TCP listener bound)
        match tokio::time::timeout(tokio::time::Duration::from_secs(30), ready_rx).await {
            Ok(Ok(Ok(()))) => {
                history::log_event(HistoryEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    event_type: "connected".to_string(),
                    connection_id: connection_id.clone(),
                    project_key: project_key.to_string(),
                    profile: profile.to_string(),
                    details: Some(format!("port {}", connection_info.port)),
                })
                .await;
                let auth_check = auth_proxy.map(|proxy| AuthCheck {
                    proxy,
                    database: connection_info.database.clone().unwrap_or_default(),
                    connection_id: connection_id.clone(),
                    connections: self.connections.clone(),
                    app_handle: self.app_handle.clone(),
                });
                // Tunnel is ready — start periodic health checks
                Self::spawn_health_check(
                    &self.app_handle,
                    &connection_id,
                    &health_port,
                    &health_cancel,
                );
                Ok((connection_id, connection_info, auth_check))
            }
            Ok(Ok(Err(e))) => {
                proxy_cancel.cancel();
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(e))
            }
            Ok(Err(_)) => {
                proxy_cancel.cancel();
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(
                    "Tunnel failed to start (channel dropped)".to_string(),
                ))
            }
            Err(_) => {
                proxy_cancel.cancel();
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(
                    "Tunnel startup timed out after 30 seconds".to_string(),
                ))
            }
        }
    }

    /// Resolve RDS target: get credentials, find bastion, get RDS endpoint.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_rds_target(
        &self,
        clients: &AwsClients,
        connection_id: &str,
        project_key: &str,
        resolved: &ResolvedEnvironment,
        profile: &str,
        project_config: &ProjectConfig,
        local_port: &str,
        selected_database: Option<&str>,
    ) -> Result<(ConnectionInfo, TunnelTarget, Option<ProxySettings>), AppError> {
        let effective_db = project_config.effective_database(selected_database);
        let environment = resolved.environment.name.as_str();
        let legacy_profiles = resolved.candidates.as_slice();

        self.emit_status("Finding bastion instance...", Some(connection_id));
        let prefs = preferences::load_preferences().await;
        let preferred =
            preferences::get_preferred_bastion(&prefs, project_key, environment, legacy_profiles)
                .map(|s| s.to_string());
        let instance_id = operations::find_bastion_instance(
            clients,
            project_config.bastion_pattern(),
            preferred.as_deref(),
        )
        .await?;

        // Save bastion preference for next time
        {
            let mut prefs = prefs;
            preferences::set_preferred_bastion(&mut prefs, project_key, environment, &instance_id);
            preferences::save_preferences(&prefs).await;
        }

        if !INSTANCE_ID_PATTERN.is_match(&instance_id) {
            return Err(AppError::Aws(format!(
                "Invalid instance ID format: {}",
                instance_id
            )));
        }

        self.emit_status("Getting RDS endpoint...", Some(connection_id));
        let rds_target = operations::get_rds_target(
            clients,
            &project_config.rds_type,
            &project_config.rds_pattern,
        )
        .await?
        .ok_or_else(|| AppError::Aws("Failed to find the RDS endpoint.".to_string()))?;
        let fallback_secret = operations::fallback_secret_name(&rds_target.identifier);
        let rds_endpoint = rds_target.endpoint;

        if !HOSTNAME_PATTERN.is_match(&rds_endpoint) {
            return Err(AppError::Aws(format!(
                "Invalid RDS endpoint format: {}",
                rds_endpoint
            )));
        }

        self.emit_status("Getting RDS port...", Some(connection_id));
        let fallback_port = get_default_port_for_engine(project_config);
        let rds_port = operations::get_rds_port(
            clients,
            &project_config.rds_type,
            &project_config.rds_pattern,
            &fallback_port,
        )
        .await?;

        let use_proxy = project_config.uses_pg_auth_proxy();
        let auth_mode = project_config.auth_mode();
        let (username, password, proxy_settings) = match auth_mode {
            AuthMode::IamOnly => {
                self.emit_status("Generating IAM auth token...", Some(connection_id));
                let iam_username = project_config.iam_username.as_deref().ok_or_else(|| {
                    AppError::Config(
                        "iamUsername is required when authType is \"iam\"".to_string(),
                    )
                })?;

                let rds_port_num: u16 = rds_port.parse().map_err(|_| {
                    AppError::General(format!("Invalid RDS port number: {}", rds_port))
                })?;

                // Build SdkConfig for SigV4 signing
                let sdk_config = build_aws_config(profile, &project_config.region).await;
                let token = iam_auth::generate_rds_auth_token(
                    &sdk_config,
                    &rds_endpoint,
                    rds_port_num,
                    iam_username,
                )
                .await?;

                if use_proxy {
                    // The token above only proves the credential chain works;
                    // the proxy mints its own on demand.
                    let local_password =
                        local_passwords::local_proxy_password(project_key, environment, legacy_profiles).await?;
                    let settings = ProxySettings {
                        db_user: iam_username.to_string(),
                        local_password: local_password.clone(),
                        auth: UpstreamAuth::IamOnly,
                        sdk_config,
                    };
                    (iam_username.to_string(), local_password, Some(settings))
                } else {
                    (iam_username.to_string(), token, None)
                }
            }
            AuthMode::Auto | AuthMode::PasswordOnly => {
                // "secrets" auth type — get credentials from Secrets Manager
                self.emit_status("Getting credentials...", Some(connection_id));
                let credentials = operations::get_connection_credentials(
                    clients,
                    &project_config.secret_prefix,
                    effective_db,
                    project_config.secret_path.as_deref(),
                    project_config.secret_username_field.as_deref(),
                    project_config.secret_password_field.as_deref(),
                    Some(&fallback_secret),
                )
                .await?;
                if credentials.via_fallback {
                    let note = format!(
                        "No secret matches '{}'; using {}",
                        project_config.secret_prefix, credentials.secret_name
                    );
                    log::info!("{}", note);
                    self.emit_status(&note, Some(connection_id));
                }
                if use_proxy {
                    let local_password =
                        local_passwords::local_proxy_password(project_key, environment, legacy_profiles).await?;
                    let settings = ProxySettings {
                        db_user: credentials.username.clone(),
                        local_password: local_password.clone(),
                        auth: {
                            let password = credentials.password;
                            let refetch = Some(SecretRefetch {
                                source: Arc::new(operations::SecretLocator {
                                    profile: profile.to_string(),
                                    region: project_config.region.clone(),
                                    secret_prefix: project_config.secret_prefix.clone(),
                                    database: effective_db.to_string(),
                                    secret_path: project_config.secret_path.clone(),
                                    username_field: project_config.secret_username_field.clone(),
                                    password_field: project_config.secret_password_field.clone(),
                                    fallback_secret: Some(fallback_secret.clone()),
                                }),
                                key: format!("{}:{}", project_key, environment),
                            });
                            if auth_mode == AuthMode::PasswordOnly {
                                UpstreamAuth::PasswordOnly { password, refetch }
                            } else {
                                UpstreamAuth::Auto { password, refetch }
                            }
                        },
                        sdk_config: build_aws_config(profile, &project_config.region).await,
                    };
                    (credentials.username, local_password, Some(settings))
                } else {
                    (credentials.username, credentials.password, None)
                }
            }
        };
        let auth_method = match (&proxy_settings, auth_mode) {
            (Some(_), AuthMode::IamOnly) => Some("IAM".to_string()),
            _ => None,
        };

        let connection_info = ConnectionInfo {
            host: "localhost".to_string(),
            port: local_port.to_string(),
            environment: environment.to_string(),
            connection_type: "rds".to_string(),
            username: Some(username),
            password: Some(password),
            database: Some(effective_db.to_string()),
            rds_endpoint: Some(rds_endpoint.clone()),
            instance_id: Some(instance_id.clone()),
            service_type: None,
            remote_host: None,
            target_type: None,
            engine: project_config.engine.clone(),
            ssh_command: None,
            auth_method,
        };

        let target = TunnelTarget::RemoteHost {
            bastion_id: instance_id,
            remote_host: rds_endpoint,
            remote_port: rds_port,
            multiplexed: project_config.multiplexed.unwrap_or(true),
        };

        Ok((connection_info, target, proxy_settings))
    }

    /// Resolve service target: find EC2/ECS target, optionally find bastion.
    async fn resolve_service_target(
        &self,
        clients: &AwsClients,
        connection_id: &str,
        project_key: &str,
        resolved: &ResolvedEnvironment,
        project_config: &ProjectConfig,
        local_port: &str,
    ) -> Result<(ConnectionInfo, TunnelTarget), AppError> {
        let environment = resolved.environment.name.as_str();
        let legacy_profiles = resolved.candidates.as_slice();
        let target_type = project_config
            .target_type
            .as_deref()
            .ok_or_else(|| AppError::Config("Missing targetType for service connection".to_string()))?;
        let remote_port = project_config
            .remote_port
            .ok_or_else(|| AppError::Config("Missing remotePort for service connection".to_string()))?;
        let target_pattern = project_config
            .target_pattern
            .as_deref()
            .unwrap_or("*");

        let (tunnel_target, instance_id, remote_host) = match target_type {
            "ec2-direct" => {
                self.emit_status("Finding target EC2 instance...", Some(connection_id));
                let (id, _ip) = operations::find_ec2_instance(clients, target_pattern).await?;

                if !INSTANCE_ID_PATTERN.is_match(&id) {
                    return Err(AppError::Aws(format!("Invalid instance ID format: {}", id)));
                }

                // Verify SSM agent is online
                self.emit_status("Checking SSM agent...", Some(connection_id));
                let ready = operations::wait_for_ssm_agent_ready(clients, &id, 3, 3000, 0).await?;
                if !ready {
                    return Err(AppError::Aws(format!(
                        "SSM agent is not online on instance {}. Ensure SSM agent is installed and running.",
                        id
                    )));
                }

                let target = TunnelTarget::DirectInstance {
                    instance_id: id.clone(),
                    remote_port: remote_port.to_string(),
                    multiplexed: project_config.multiplexed.unwrap_or(true),
                };
                (target, Some(id), None)
            }
            "ec2-bastion" => {
                self.emit_status("Finding bastion instance...", Some(connection_id));
                let prefs = preferences::load_preferences().await;
                let preferred = preferences::get_preferred_bastion(
                    &prefs,
                    project_key,
                    environment,
                    legacy_profiles,
                )
                .map(|s| s.to_string());
                let bastion_id = operations::find_bastion_instance(
                    clients,
                    project_config.bastion_pattern(),
                    preferred.as_deref(),
                )
                .await?;

                // Save bastion preference
                {
                    let mut prefs = prefs;
                    preferences::set_preferred_bastion(&mut prefs, project_key, environment, &bastion_id);
                    preferences::save_preferences(&prefs).await;
                }

                if !INSTANCE_ID_PATTERN.is_match(&bastion_id) {
                    return Err(AppError::Aws(format!("Invalid bastion instance ID format: {}", bastion_id)));
                }

                self.emit_status("Finding target EC2 instance...", Some(connection_id));
                let (_id, ip) = operations::find_ec2_instance(clients, target_pattern).await?;

                if !IP_PATTERN.is_match(&ip) {
                    return Err(AppError::Aws(format!("Invalid EC2 private IP format: {}", ip)));
                }

                let target = TunnelTarget::RemoteHost {
                    bastion_id: bastion_id.clone(),
                    remote_host: ip.clone(),
                    remote_port: remote_port.to_string(),
                    multiplexed: project_config.multiplexed.unwrap_or(true),
                };
                (target, Some(bastion_id), Some(ip))
            }
            "ecs-bastion" => {
                let cluster = project_config
                    .ecs_cluster
                    .as_deref()
                    .ok_or_else(|| AppError::Config("Missing ecsCluster for ecs-bastion connection".to_string()))?;
                let service = project_config
                    .ecs_service
                    .as_deref()
                    .ok_or_else(|| AppError::Config("Missing ecsService for ecs-bastion connection".to_string()))?;

                self.emit_status("Finding bastion instance...", Some(connection_id));
                let prefs = preferences::load_preferences().await;
                let preferred = preferences::get_preferred_bastion(
                    &prefs,
                    project_key,
                    environment,
                    legacy_profiles,
                )
                .map(|s| s.to_string());
                let bastion_id = operations::find_bastion_instance(
                    clients,
                    project_config.bastion_pattern(),
                    preferred.as_deref(),
                )
                .await?;

                // Save bastion preference
                {
                    let mut prefs = prefs;
                    preferences::set_preferred_bastion(&mut prefs, project_key, environment, &bastion_id);
                    preferences::save_preferences(&prefs).await;
                }

                if !INSTANCE_ID_PATTERN.is_match(&bastion_id) {
                    return Err(AppError::Aws(format!("Invalid bastion instance ID format: {}", bastion_id)));
                }

                self.emit_status("Finding ECS task IP...", Some(connection_id));
                let task_ip = operations::find_ecs_task_ip(clients, cluster, service).await?;

                if !IP_PATTERN.is_match(&task_ip) {
                    return Err(AppError::Aws(format!("Invalid ECS task IP format: {}", task_ip)));
                }

                let target = TunnelTarget::RemoteHost {
                    bastion_id: bastion_id.clone(),
                    remote_host: task_ip.clone(),
                    remote_port: remote_port.to_string(),
                    multiplexed: project_config.multiplexed.unwrap_or(true),
                };
                (target, Some(bastion_id), Some(task_ip))
            }
            _ => return Err(AppError::Config(format!("Unknown targetType: {}", target_type))),
        };

        // Build SSH command if service type is SSH
        let ssh_command = if project_config.service_type.as_deref() == Some("ssh") {
            let ssh_user = project_config
                .ssh_username
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("ec2-user");
            let mut cmd = format!(
                "ssh -p {} {}@localhost -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null",
                local_port, ssh_user
            );
            if let Some(ref key_path) = project_config.ssh_key_path
                && !key_path.is_empty()
            {
                cmd.push_str(&format!(" -i {}", key_path));
            }
            Some(cmd)
        } else {
            None
        };

        let connection_info = ConnectionInfo {
            host: "localhost".to_string(),
            port: local_port.to_string(),
            environment: environment.to_string(),
            connection_type: "service".to_string(),
            username: None,
            password: None,
            database: None,
            rds_endpoint: None,
            instance_id,
            service_type: project_config.service_type.clone(),
            remote_host,
            target_type: Some(target_type.to_string()),
            engine: None,
            ssh_command,
            auth_method: None,
        };

        Ok((connection_info, tunnel_target))
    }

    /// Disconnect a specific connection.
    pub async fn disconnect(&self, connection_id: &str) -> Result<(), AppError> {
        let mut guard = self.connections.lock().await;
        if let Some(connection) = guard.remove(connection_id) {
            connection.cancel_token.cancel();
        }
        Ok(())
    }

    /// Disconnect all connections.
    pub async fn disconnect_all(&self) -> Result<(), AppError> {
        let mut guard = self.connections.lock().await;
        for (_id, connection) in guard.drain() {
            connection.cancel_token.cancel();
        }
        Ok(())
    }

    /// Spawn a background health check task for a connection.
    /// Periodically verifies the tunnel's local listener is still bound (via a
    /// bind-conflict probe — never by connecting, which would create tunnel traffic).
    fn spawn_health_check(
        app_handle: &AppHandle,
        connection_id: &str,
        local_port: &str,
        cancel_token: &CancellationToken,
    ) {
        let app_handle = app_handle.clone();
        let conn_id = connection_id.to_string();
        let port: u16 = local_port.parse().unwrap_or(0);
        let cancel = cancel_token.clone();

        tokio::spawn(async move {
            // Initial delay: wait for tunnel to fully establish before first check
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS)) => {}
                _ = cancel.cancelled() => { return; }
            }

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                // Check that the tunnel's local listener is still bound WITHOUT
                // opening a TCP connection to it. Connecting (the old approach)
                // was actively harmful in multiplexed mode: every probe opened a
                // real smux stream, which made the agent dial the remote host
                // (RDS/VNC) and tear it down again — every 30 seconds, for the
                // lifetime of every connection. A bind attempt gives the same
                // signal (listener present) with zero tunnel traffic: if the
                // port is held by our listener, bind fails with AddrInUse.
                let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
                let status = match std::net::TcpListener::bind(addr) {
                    // Bind succeeded — nothing is listening on the port anymore.
                    Ok(sock) => {
                        drop(sock);
                        "unhealthy"
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => "healthy",
                    // Unexpected bind error (permissions etc.) — can't conclude.
                    Err(_) => "degraded",
                };

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                let _ = app_handle.emit(
                    "connection-health",
                    serde_json::json!({
                        "connectionId": conn_id,
                        "status": status,
                        "lastCheck": now_ms,
                    }),
                );

                // Wait for next interval or cancellation
                tokio::select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS)) => {}
                    _ = cancel.cancelled() => { break; }
                }
            }
        });
    }

    fn emit_status(&self, message: &str, connection_id: Option<&str>) {
        let mut payload = serde_json::json!({ "message": message });
        if let Some(id) = connection_id {
            payload["connectionId"] = serde_json::json!(id);
        }
        let _ = self.app_handle.emit("status", &payload);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ActiveConnectionInfo {
    pub id: String,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    #[serde(default)]
    pub environment: String,
    pub profile: String,
    #[serde(rename = "localPort")]
    pub local_port: String,
    #[serde(rename = "connectionInfo")]
    pub connection_info: ConnectionInfo,
    pub status: String,
}

/// Describes what the tunnel connects to.
#[derive(Clone)]
enum TunnelTarget {
    /// Port forwarding through a bastion to a remote host (RDS, EC2 via bastion, ECS via bastion).
    RemoteHost {
        bastion_id: String,
        remote_host: String,
        remote_port: String,
        multiplexed: bool,
    },
    /// Direct port forwarding to an EC2 instance (SSM agent on the instance itself).
    DirectInstance {
        instance_id: String,
        remote_port: String,
        multiplexed: bool,
    },
}

impl TunnelTarget {
    fn is_multiplexed(&self) -> bool {
        match self {
            TunnelTarget::RemoteHost { multiplexed, .. } => *multiplexed,
            TunnelTarget::DirectInstance { multiplexed, .. } => *multiplexed,
        }
    }
}

/// Run the tunnel lifecycle: start port forwarding, keepalive, auto-reconnect.
#[allow(clippy::too_many_arguments)]
async fn run_tunnel_lifecycle(
    app_handle: &AppHandle,
    clients: &AwsClients,
    connection_id: &str,
    local_port: &str,
    project_config: &ProjectConfig,
    mut target: TunnelTarget,
    pipe_inbox: Option<PipeInbox>,
    cancel_token: CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    conn_label: &str,
    project_key: &str,
    profile: &str,
) -> Result<(), AppError> {
    let mut reconnect_count: u32 = 0;
    let mut ready_tx = ready_tx;

    loop {
        if cancel_token.is_cancelled() {
            break;
        }

        let result = start_port_forwarding_with_retry(
            clients,
            local_port,
            &target,
            pipe_inbox.as_ref(),
            project_config.bastion_pattern(),
            &cancel_token,
            ready_tx.take(),
        )
        .await;

        if cancel_token.is_cancelled() {
            break;
        }

        // Notify about connection drop on first failure in this cycle
        if reconnect_count == 0 {
            send_notification(
                app_handle,
                "Connection Lost",
                &format!("{} — attempting to reconnect...", conn_label),
            );
        }

        // Handle reconnect
        match result {
            Ok(()) => {
                reconnect_count += 1;
                if reconnect_count > AUTO_RECONNECT_MAX_RETRIES {
                    return Err(AppError::Tunnel(
                        "Maximum auto-reconnection attempts reached.".to_string(),
                    ));
                }
                history::log_event(HistoryEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    event_type: "reconnected".to_string(),
                    connection_id: connection_id.to_string(),
                    project_key: project_key.to_string(),
                    profile: profile.to_string(),
                    details: Some(format!("attempt {}/{}", reconnect_count, AUTO_RECONNECT_MAX_RETRIES)),
                })
                .await;
                emit_status_event(
                    app_handle,
                    &format!("Session ended. Reconnecting... ({})", reconnect_count),
                    Some(connection_id),
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(AUTO_RECONNECT_DELAY_MS))
                    .await;
            }
            Err(e) => {
                if cancel_token.is_cancelled() {
                    break;
                }
                reconnect_count += 1;
                if reconnect_count > AUTO_RECONNECT_MAX_RETRIES {
                    return Err(e);
                }
                history::log_event(HistoryEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    event_type: "reconnected".to_string(),
                    connection_id: connection_id.to_string(),
                    project_key: project_key.to_string(),
                    profile: profile.to_string(),
                    details: Some(format!("error retry {}/{}: {}", reconnect_count, AUTO_RECONNECT_MAX_RETRIES, e)),
                })
                .await;
                emit_status_event(
                    app_handle,
                    &format!(
                        "Connection error. Retrying... ({}/{})",
                        reconnect_count, AUTO_RECONNECT_MAX_RETRIES
                    ),
                    Some(connection_id),
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(AUTO_RECONNECT_DELAY_MS * 2))
                    .await;
            }
        }

        if cancel_token.is_cancelled() {
            break;
        }

        // Verify credentials
        emit_status_event(app_handle, "Checking credentials...", Some(connection_id));
        let cred_check = operations::check_credentials_valid(clients).await;
        if !cred_check.valid {
            emit_status_event(
                app_handle,
                "AWS credentials expired. Please re-authenticate and reconnect.",
                Some(connection_id),
            );
            break;
        }

        // Re-discover infrastructure based on target type
        target = rediscover_target(app_handle, clients, connection_id, project_config, &target).await?;

        // Notify that auto-reconnect succeeded
        send_notification(
            app_handle,
            "Reconnected",
            &format!("{} tunnel restored", conn_label),
        );

        // Reset reconnect counter for the next cycle
        reconnect_count = 0;

        emit_status_event(
            app_handle,
            "Reconnecting port forwarding...",
            Some(connection_id),
        );
    }

    Ok(())
}

/// Re-discover infrastructure for reconnection.
async fn rediscover_target(
    app_handle: &AppHandle,
    clients: &AwsClients,
    connection_id: &str,
    project_config: &ProjectConfig,
    current: &TunnelTarget,
) -> Result<TunnelTarget, AppError> {
    match current {
        TunnelTarget::RemoteHost { remote_port, multiplexed, .. } => {
            // Re-find bastion (no preference on reconnect — use whatever is available)
            emit_status_event(app_handle, "Finding bastion instance...", Some(connection_id));
            let bastion_id =
                operations::find_bastion_instance(clients, project_config.bastion_pattern(), None).await?;

            // Re-discover remote host based on connection type
            let remote_host = if project_config.connection_type == "rds" {
                emit_status_event(app_handle, "Getting RDS endpoint...", Some(connection_id));
                operations::get_rds_endpoint(
                    clients,
                    &project_config.rds_type,
                    &project_config.rds_pattern,
                )
                .await?
                .ok_or_else(|| {
                    AppError::Aws("Failed to find the RDS endpoint during reconnection.".to_string())
                })?
            } else if project_config.target_type.as_deref() == Some("ecs-bastion") {
                emit_status_event(app_handle, "Finding ECS task IP...", Some(connection_id));
                operations::find_ecs_task_ip(
                    clients,
                    project_config.ecs_cluster.as_deref().unwrap_or(""),
                    project_config.ecs_service.as_deref().unwrap_or(""),
                )
                .await?
            } else {
                // ec2-bastion: re-find the EC2 instance IP
                emit_status_event(app_handle, "Finding target instance...", Some(connection_id));
                let (_id, ip) = operations::find_ec2_instance(
                    clients,
                    project_config.target_pattern.as_deref().unwrap_or("*"),
                )
                .await?;
                ip
            };

            Ok(TunnelTarget::RemoteHost {
                bastion_id,
                remote_host,
                remote_port: remote_port.clone(),
                multiplexed: *multiplexed,
            })
        }
        TunnelTarget::DirectInstance { remote_port, multiplexed, .. } => {
            // Re-find the direct instance
            emit_status_event(app_handle, "Finding target instance...", Some(connection_id));
            let (instance_id, _ip) = operations::find_ec2_instance(
                clients,
                project_config.target_pattern.as_deref().unwrap_or("*"),
            )
            .await?;
            Ok(TunnelTarget::DirectInstance {
                instance_id,
                remote_port: remote_port.clone(),
                multiplexed: *multiplexed,
            })
        }
    }
}

/// Start port forwarding with TargetNotConnected retry.
async fn start_port_forwarding_with_retry(
    clients: &AwsClients,
    local_port: &str,
    target: &TunnelTarget,
    pipe_inbox: Option<&PipeInbox>,
    bastion_pattern: &str,
    cancel_token: &CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), AppError> {
    let mut current_target = target.clone();
    let mut retry_count: u32 = 0;
    let mut ready_tx = ready_tx;

    loop {
        let result = execute_port_forwarding(
            clients,
            local_port,
            &current_target,
            pipe_inbox,
            cancel_token,
            ready_tx.take(),
        )
        .await;

        match result {
            Ok(()) => return Ok(()),
            Err(PortForwardError::TargetNotConnected) if retry_count < PORT_FORWARDING_MAX_RETRIES => {
                if let TunnelTarget::RemoteHost { ref remote_host, ref remote_port, multiplexed, .. } = current_target {
                    // TargetNotConnected means the SSM target isn't currently reachable —
                    // typically a stale/cached bastion ID pointing at a replaced instance, or
                    // the SSM agent briefly not connected. Re-discover the current running
                    // bastion by tag and retry.
                    //
                    // We must NEVER terminate the bastion here. The bastion is a shared,
                    // externally-managed instance (e.g. behind an Auto Scaling Group) used by
                    // every developer. Terminating it drops all other users' tunnels at once,
                    // and — because their apps then also hit TargetNotConnected — can cascade
                    // into repeatedly killing each ASG replacement. Discovery alone is correct
                    // and self-heals an ASG-driven instance swap.
                    let new_id =
                        operations::find_bastion_instance(clients, bastion_pattern, None).await?;
                    current_target = TunnelTarget::RemoteHost {
                        bastion_id: new_id,
                        remote_host: remote_host.clone(),
                        remote_port: remote_port.clone(),
                        multiplexed,
                    };
                }

                retry_count += 1;
            }
            Err(PortForwardError::TargetNotConnected) => {
                return Err(AppError::Tunnel(
                    "TargetNotConnected: max retries exceeded".to_string(),
                ));
            }
            Err(PortForwardError::Cancelled) => return Ok(()),
            Err(PortForwardError::Failed(msg)) => return Err(AppError::Tunnel(msg)),
        }
    }
}

#[derive(Clone)]
enum PortForwardError {
    TargetNotConnected,
    Cancelled,
    Failed(String),
}

/// Execute a single port forwarding session via native WebSocket.
async fn execute_port_forwarding(
    clients: &AwsClients,
    local_port: &str,
    target: &TunnelTarget,
    pipe_inbox: Option<&PipeInbox>,
    cancel_token: &CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), PortForwardError> {
    // Start SSM session based on target type
    let session_response = match target {
        TunnelTarget::RemoteHost { bastion_id, remote_host, remote_port, .. } => {
            operations::start_remote_port_forwarding_session(
                clients, bastion_id, remote_host, remote_port, local_port,
            )
            .await
        }
        TunnelTarget::DirectInstance { instance_id, remote_port, .. } => {
            operations::start_direct_port_forwarding_session(
                clients, instance_id, remote_port, local_port,
            )
            .await
        }
    }
    .map_err(|e| PortForwardError::Failed(e.to_string()))?;

    let stream_url = session_response
        .stream_url()
        .ok_or_else(|| PortForwardError::Failed("No StreamUrl in session response".to_string()))?
        .to_string();
    let token_value = session_response
        .token_value()
        .ok_or_else(|| PortForwardError::Failed("No TokenValue in session response".to_string()))?
        .to_string();

    let port_num: u16 = local_port
        .parse()
        .map_err(|_| PortForwardError::Failed(format!("Invalid port: {}", local_port)))?;

    let multiplexed = target.is_multiplexed();
    let local = match pipe_inbox {
        Some(inbox) => LocalSource::Pipes(inbox.clone()),
        None => LocalSource::Listen(port_num),
    };

    // Run port forwarding — multiplexed or basic mode
    let cancel_child = cancel_token.child_token();
    let result = if multiplexed {
        log::info!("Starting multiplexed port forwarding on port {}", port_num);
        native::start_multiplexed_port_forwarding(
            stream_url,
            token_value,
            local,
            cancel_child,
            ready_tx,
        )
        .await
    } else {
        native::start_native_port_forwarding(
            stream_url,
            token_value,
            local,
            cancel_child,
            ready_tx,
        )
        .await
    };

    if cancel_token.is_cancelled() {
        return Err(PortForwardError::Cancelled);
    }

    match result {
        Ok(()) => Ok(()),
        Err(msg) if msg.contains("TargetNotConnected") || msg.contains("is not connected") => {
            Err(PortForwardError::TargetNotConnected)
        }
        Err(msg) => Err(PortForwardError::Failed(msg)),
    }
}

/// Send a desktop notification only when no app window is focused.
/// Failures are silently ignored so notifications are never fatal.
fn send_notification(app_handle: &AppHandle, title: &str, body: &str) {
    // Skip notification if any app window is currently focused
    if app_handle.webview_windows().values().any(|w| w.is_focused().unwrap_or(false)) {
        return;
    }
    let _ = app_handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

fn emit_status_event(app_handle: &AppHandle, message: &str, connection_id: Option<&str>) {
    let mut payload = serde_json::json!({ "message": message });
    if let Some(id) = connection_id {
        payload["connectionId"] = serde_json::json!(id);
    }
    let _ = app_handle.emit("status", &payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(id: &str) -> Connection {
        Connection {
            id: id.to_string(),
            project_key: "covered".to_string(),
            environment: "dev".to_string(),
            profile: "covered-dev".to_string(),
            local_port: "5462".to_string(),
            connection_info: ConnectionInfo {
                host: "localhost".to_string(),
                port: "5462".to_string(),
                environment: "dev".to_string(),
                connection_type: "rds".to_string(),
                username: Some("covered_app".to_string()),
                password: Some("local".to_string()),
                database: Some("covered_db".to_string()),
                rds_endpoint: None,
                instance_id: None,
                service_type: None,
                remote_host: None,
                target_type: None,
                engine: None,
                ssh_command: None,
                auth_method: None,
            },
            cancel_token: CancellationToken::new(),
        }
    }

    #[test]
    fn auth_method_is_recorded_on_a_live_connection() {
        let mut connections = HashMap::from([("conn_1".to_string(), connection("conn_1"))]);
        assert_eq!(
            record_auth_method(&mut connections, "conn_1", "IAM (auto)".to_string()).as_deref(),
            Some("IAM (auto)")
        );
        assert_eq!(connections["conn_1"].connection_info.auth_method.as_deref(), Some("IAM (auto)"));
    }

    #[test]
    fn a_connection_closed_during_the_check_is_reported() {
        let mut connections = HashMap::new();
        assert_eq!(record_auth_method(&mut connections, "conn_1", "password".to_string()), None);
    }
}
