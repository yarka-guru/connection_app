use crate::aws::credentials::{build_aws_config, create_aws_clients, AwsClients};
use crate::aws::iam_auth;
use crate::aws::operations;
use crate::aws::sso::{ensure_sso_session, TauriSsoHandler};
use crate::config::preferences;
use crate::config::projects::{
    get_default_port_for_engine, get_local_port, load_project_configs, ProjectConfig,
};
use crate::error::AppError;
use crate::history::{self, HistoryEntry};
use crate::tunnel::native;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// Retry configuration constants (matching connect.js)
const BASTION_WAIT_MAX_RETRIES: u32 = 20;
const BASTION_WAIT_RETRY_DELAY_MS: u64 = 15000;
const PORT_FORWARDING_MAX_RETRIES: u32 = 2;
const AUTO_RECONNECT_MAX_RETRIES: u32 = 3;
const AUTO_RECONNECT_DELAY_MS: u64 = 3000;

// Health check constants
const HEALTH_CHECK_INTERVAL_SECS: u64 = 30;
const HEALTH_CHECK_TIMEOUT_MS: u64 = 5000;
const HEALTH_CHECK_DEGRADED_MS: u64 = 5000;

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
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub id: String,
    pub project_key: String,
    pub profile: String,
    pub local_port: String,
    pub connection_info: ConnectionInfo,
    cancel_token: CancellationToken,
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
                profile: c.profile.clone(),
                local_port: c.local_port.clone(),
                connection_info: c.connection_info.clone(),
                status: "connected".to_string(),
            })
            .collect()
    }

    /// Connect to a project (dispatches to RDS or service based on connectionType).
    pub async fn connect(
        &self,
        project_key: &str,
        profile: &str,
        local_port: Option<&str>,
        database: Option<&str>,
        used_ports: &[String],
    ) -> Result<(String, ConnectionInfo), AppError> {
        // Validate profile
        if !PROFILE_SAFE_PATTERN.is_match(profile) {
            return Err(AppError::General(format!(
                "Invalid profile name: {}",
                profile
            )));
        }

        // Load project config
        let configs = load_project_configs().await?;
        let project_config = configs.get(project_key).ok_or_else(|| {
            AppError::Config(format!("Unknown project: {}", project_key))
        })?;

        // Determine port
        let requested_port = local_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| get_local_port(profile, project_config));

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

        // Dispatch based on connection type
        let (connection_info, tunnel_target) = match project_config.connection_type.as_str() {
            "service" => self.resolve_service_target(&clients, &connection_id, project_key, profile, project_config, &port_to_use).await?,
            _ => self.resolve_rds_target(&clients, &connection_id, project_key, profile, project_config, &port_to_use, database).await?,
        };

        let cancel_token = CancellationToken::new();

        let connection = Connection {
            id: connection_id.clone(),
            project_key: project_key.to_string(),
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
        let project_config = project_config.clone();

        tokio::spawn(async move {
            let conn_label = format!("{} {}", project_key_owned, profile_owned);
            let result = run_tunnel_lifecycle(
                &app_handle,
                &clients,
                &conn_id,
                &port_to_use,
                &project_config,
                tunnel_target,
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
                // Tunnel is ready — start periodic health checks
                Self::spawn_health_check(
                    &self.app_handle,
                    &connection_id,
                    &health_port,
                    &health_cancel,
                );
                Ok((connection_id, connection_info))
            }
            Ok(Ok(Err(e))) => {
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(e))
            }
            Ok(Err(_)) => {
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(
                    "Tunnel failed to start (channel dropped)".to_string(),
                ))
            }
            Err(_) => {
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
        profile: &str,
        project_config: &ProjectConfig,
        local_port: &str,
        selected_database: Option<&str>,
    ) -> Result<(ConnectionInfo, TunnelTarget), AppError> {
        let effective_db = project_config.effective_database(selected_database);

        self.emit_status("Finding bastion instance...", Some(connection_id));
        let prefs = preferences::load_preferences().await;
        let preferred = preferences::get_preferred_bastion(&prefs, project_key, profile)
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
            preferences::set_preferred_bastion(&mut prefs, project_key, profile, &instance_id);
            preferences::save_preferences(&prefs).await;
        }

        if !INSTANCE_ID_PATTERN.is_match(&instance_id) {
            return Err(AppError::Aws(format!(
                "Invalid instance ID format: {}",
                instance_id
            )));
        }

        self.emit_status("Getting RDS endpoint...", Some(connection_id));
        let rds_endpoint = operations::get_rds_endpoint(
            clients,
            &project_config.rds_type,
            &project_config.rds_pattern,
        )
        .await?
        .ok_or_else(|| AppError::Aws("Failed to find the RDS endpoint.".to_string()))?;

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

        // Determine auth type (default to "secrets")
        let auth_type = if project_config.auth_type.is_empty() {
            "secrets"
        } else {
            project_config.auth_type.as_str()
        };

        let (username, password) = match auth_type {
            "iam" => {
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

                (iam_username.to_string(), token)
            }
            _ => {
                // "secrets" auth type — get credentials from Secrets Manager
                self.emit_status("Getting credentials...", Some(connection_id));
                let credentials = operations::get_connection_credentials(
                    clients,
                    &project_config.secret_prefix,
                    effective_db,
                    project_config.secret_path.as_deref(),
                    project_config.secret_username_field.as_deref(),
                    project_config.secret_password_field.as_deref(),
                )
                .await?;
                (credentials.username, credentials.password)
            }
        };

        let connection_info = ConnectionInfo {
            host: "localhost".to_string(),
            port: local_port.to_string(),
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
        };

        let target = TunnelTarget::RemoteHost {
            bastion_id: instance_id,
            remote_host: rds_endpoint,
            remote_port: rds_port,
            multiplexed: project_config.multiplexed.unwrap_or(false),
        };

        Ok((connection_info, target))
    }

    /// Resolve service target: find EC2/ECS target, optionally find bastion.
    async fn resolve_service_target(
        &self,
        clients: &AwsClients,
        connection_id: &str,
        project_key: &str,
        profile: &str,
        project_config: &ProjectConfig,
        local_port: &str,
    ) -> Result<(ConnectionInfo, TunnelTarget), AppError> {
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
                    multiplexed: project_config.multiplexed.unwrap_or(false),
                };
                (target, Some(id), None)
            }
            "ec2-bastion" => {
                self.emit_status("Finding bastion instance...", Some(connection_id));
                let prefs = preferences::load_preferences().await;
                let preferred = preferences::get_preferred_bastion(&prefs, project_key, profile)
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
                    preferences::set_preferred_bastion(&mut prefs, project_key, profile, &bastion_id);
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
                    multiplexed: project_config.multiplexed.unwrap_or(false),
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
                let preferred = preferences::get_preferred_bastion(&prefs, project_key, profile)
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
                    preferences::set_preferred_bastion(&mut prefs, project_key, profile, &bastion_id);
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
                    multiplexed: project_config.multiplexed.unwrap_or(false),
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
    /// Periodically attempts a TCP connect to localhost:port to verify the tunnel is alive.
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

                let check_start = std::time::Instant::now();
                let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));

                let status = match tokio::time::timeout(
                    tokio::time::Duration::from_millis(HEALTH_CHECK_TIMEOUT_MS),
                    tokio::net::TcpStream::connect(addr),
                )
                .await
                {
                    Ok(Ok(_stream)) => {
                        let elapsed = check_start.elapsed().as_millis() as u64;
                        if elapsed > HEALTH_CHECK_DEGRADED_MS {
                            "degraded"
                        } else {
                            "healthy"
                        }
                    }
                    Ok(Err(_)) => "unhealthy",
                    Err(_) => "unhealthy", // timeout
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
            project_config.bastion_pattern(),
            project_config.connection_type.as_str(),
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
    bastion_pattern: &str,
    connection_type: &str,
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
            cancel_token,
            ready_tx.take(),
        )
        .await;

        match result {
            Ok(()) => return Ok(()),
            Err(PortForwardError::TargetNotConnected) if retry_count < PORT_FORWARDING_MAX_RETRIES => {
                if let TunnelTarget::RemoteHost { ref bastion_id, ref remote_host, ref remote_port, multiplexed } = current_target {
                    if connection_type == "rds" {
                        let _ = operations::terminate_bastion_instance(clients, bastion_id).await;

                        let new_id = operations::wait_for_new_bastion_instance(
                            clients,
                            bastion_id,
                            bastion_pattern,
                            BASTION_WAIT_MAX_RETRIES,
                            BASTION_WAIT_RETRY_DELAY_MS,
                        )
                        .await?
                        .ok_or_else(|| {
                            AppError::Tunnel(
                                "Failed to find new bastion instance after waiting.".to_string(),
                            )
                        })?;

                        current_target = TunnelTarget::RemoteHost {
                            bastion_id: new_id,
                            remote_host: remote_host.clone(),
                            remote_port: remote_port.clone(),
                            multiplexed,
                        };
                    } else {
                        let new_id = operations::find_bastion_instance(clients, bastion_pattern, None).await?;
                        current_target = TunnelTarget::RemoteHost {
                            bastion_id: new_id,
                            remote_host: remote_host.clone(),
                            remote_port: remote_port.clone(),
                            multiplexed,
                        };
                    }
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

    // Run port forwarding — multiplexed or basic mode
    let cancel_child = cancel_token.child_token();
    let result = if multiplexed {
        log::info!("Starting multiplexed port forwarding on port {}", port_num);
        native::start_multiplexed_port_forwarding(
            stream_url,
            token_value,
            port_num,
            cancel_child,
            ready_tx,
        )
        .await
    } else {
        native::start_native_port_forwarding(
            stream_url,
            token_value,
            port_num,
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
