use crate::aws::credentials::{create_aws_clients, AwsClients};
use crate::aws::operations;
use crate::aws::sso::{ensure_sso_session, TauriSsoHandler};
use crate::config::projects::{
    get_default_port_for_engine, get_local_port, load_project_configs, ProjectConfig,
};
use crate::error::AppError;
use crate::tunnel::native;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// Retry configuration constants (matching connect.js)
const BASTION_WAIT_MAX_RETRIES: u32 = 20;
const BASTION_WAIT_RETRY_DELAY_MS: u64 = 15000;
const PORT_FORWARDING_MAX_RETRIES: u32 = 2;
const AUTO_RECONNECT_MAX_RETRIES: u32 = 3;
const AUTO_RECONNECT_DELAY_MS: u64 = 3000;

// Validation patterns
static PROFILE_SAFE_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._-]+$").unwrap());
static INSTANCE_ID_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^i-[a-f0-9]{8,17}$").unwrap());
static HOSTNAME_PATTERN: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9.-]+$").unwrap());

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConnectionInfo {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
    #[serde(rename = "rdsEndpoint")]
    pub rds_endpoint: Option<String>,
    #[serde(rename = "instanceId")]
    pub instance_id: Option<String>,
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

impl TunnelManager {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            app_handle,
        }
    }

    /// Check if a port is available by attempting to bind.
    async fn is_port_available(port: u16) -> bool {
        tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .is_ok()
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

    /// Connect to RDS through bastion.
    pub async fn connect(
        &self,
        project_key: &str,
        profile: &str,
        local_port: Option<&str>,
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
        let port_to_use = local_port
            .map(|p| p.to_string())
            .unwrap_or_else(|| get_local_port(profile, project_config));

        let port_num: u16 = port_to_use
            .parse()
            .map_err(|_| AppError::General(format!("Invalid port number: {}", port_to_use)))?;

        // Strict port check
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

        if all_used_ports.contains(&port_num) || !Self::is_port_available(port_num).await {
            return Err(AppError::Tunnel(format!(
                "Port {} is not available. Close the application using it or change the port in project settings.",
                port_to_use
            )));
        }

        // Generate connection ID
        let connection_id = format!("conn_{}", &uuid::Uuid::new_v4().to_string()[..8]);

        // SSO pre-flight
        let sso_handler = TauriSsoHandler {
            app_handle: self.app_handle.clone(),
        };
        ensure_sso_session(profile, &sso_handler, Some(&connection_id)).await?;

        // Create AWS clients
        let clients = create_aws_clients(profile, &project_config.region).await;

        // Get credentials
        self.emit_status("Getting credentials...", Some(&connection_id));
        let credentials = operations::get_connection_credentials(
            &clients,
            &project_config.secret_prefix,
            &project_config.database,
        )
        .await?;

        // Find bastion
        self.emit_status("Finding bastion instance...", Some(&connection_id));
        let instance_id = operations::find_bastion_instance(&clients).await?;

        if !INSTANCE_ID_PATTERN.is_match(&instance_id) {
            return Err(AppError::Aws(format!(
                "Invalid instance ID format: {}",
                instance_id
            )));
        }

        // Get RDS endpoint
        self.emit_status("Getting RDS endpoint...", Some(&connection_id));
        let rds_endpoint = operations::get_rds_endpoint(
            &clients,
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

        // Get RDS port
        self.emit_status("Getting RDS port...", Some(&connection_id));
        let fallback_port = get_default_port_for_engine(project_config);
        let rds_port = operations::get_rds_port(
            &clients,
            &project_config.rds_type,
            &project_config.rds_pattern,
            &fallback_port,
        )
        .await?;

        let connection_info = ConnectionInfo {
            host: "localhost".to_string(),
            port: port_to_use.clone(),
            username: credentials.username.clone(),
            password: credentials.password.clone(),
            database: project_config.database.clone(),
            rds_endpoint: Some(rds_endpoint.clone()),
            instance_id: Some(instance_id.clone()),
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

        // Spawn background task for port forwarding lifecycle
        let app_handle = self.app_handle.clone();
        let connections = self.connections.clone();
        let conn_id = connection_id.clone();
        let project_config = project_config.clone();

        tokio::spawn(async move {
            let result = run_tunnel_lifecycle(
                &app_handle,
                &clients,
                &conn_id,
                &instance_id,
                &rds_endpoint,
                &port_to_use,
                &rds_port,
                &project_config,
                cancel_token,
                Some(ready_tx),
            )
            .await;

            // Clean up connection
            {
                let mut guard = connections.lock().await;
                guard.remove(&conn_id);
            }

            match result {
                Ok(()) => {
                    let _ = app_handle.emit(
                        "disconnected",
                        serde_json::json!({
                            "connectionId": conn_id,
                            "reason": "session_ended"
                        }),
                    );
                }
                Err(e) => {
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
                // Tunnel is ready
                Ok((connection_id, connection_info))
            }
            Ok(Ok(Err(e))) => {
                // Tunnel failed to start
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(e))
            }
            Ok(Err(_)) => {
                // Channel dropped — tunnel task failed before signaling
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(
                    "Tunnel failed to start (channel dropped)".to_string(),
                ))
            }
            Err(_) => {
                // Timeout
                let mut guard = self.connections.lock().await;
                guard.remove(&connection_id);
                Err(AppError::Tunnel(
                    "Tunnel startup timed out after 30 seconds".to_string(),
                ))
            }
        }
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

/// Run the tunnel lifecycle: start port forwarding, keepalive, auto-reconnect.
#[allow(clippy::too_many_arguments)]
async fn run_tunnel_lifecycle(
    app_handle: &AppHandle,
    clients: &AwsClients,
    connection_id: &str,
    initial_instance_id: &str,
    initial_rds_endpoint: &str,
    local_port: &str,
    rds_port: &str,
    project_config: &ProjectConfig,
    cancel_token: CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), AppError> {
    let mut current_instance_id = initial_instance_id.to_string();
    let mut current_rds_endpoint = initial_rds_endpoint.to_string();
    let mut reconnect_count: u32 = 0;
    let mut ready_tx = ready_tx;

    loop {
        if cancel_token.is_cancelled() {
            break;
        }

        // Start port forwarding (pass ready_tx only on first attempt)
        // Note: WebSocket-level pings (30s interval in native.rs) handle keepalive.
        // The old TCP-connect keepalive was counterproductive — it created full
        // connect/disconnect cycles on the remote port every 4 minutes.
        let result = start_port_forwarding_with_retry(
            clients,
            &current_instance_id,
            &current_rds_endpoint,
            local_port,
            rds_port,
            &project_config.region,
            &cancel_token,
            ready_tx.take(),
        )
        .await;

        if cancel_token.is_cancelled() {
            break;
        }

        match result {
            Ok(()) => {
                // Session ended cleanly (unexpected disconnect)
                reconnect_count += 1;
                if reconnect_count > AUTO_RECONNECT_MAX_RETRIES {
                    return Err(AppError::Tunnel(
                        "Maximum auto-reconnection attempts reached.".to_string(),
                    ));
                }

                emit_status_event(
                    app_handle,
                    &format!("Session ended. Reconnecting... ({})", reconnect_count),
                    Some(connection_id),
                );

                tokio::time::sleep(tokio::time::Duration::from_millis(AUTO_RECONNECT_DELAY_MS))
                    .await;

                if cancel_token.is_cancelled() {
                    break;
                }

                // Verify credentials
                emit_status_event(
                    app_handle,
                    "Checking credentials...",
                    Some(connection_id),
                );
                let cred_check = operations::check_credentials_valid(clients).await;
                if !cred_check.valid {
                    emit_status_event(
                        app_handle,
                        "AWS credentials expired. Please re-authenticate and reconnect.",
                        Some(connection_id),
                    );
                    break;
                }

                // Re-discover infrastructure
                emit_status_event(
                    app_handle,
                    "Finding bastion instance...",
                    Some(connection_id),
                );
                current_instance_id = operations::find_bastion_instance(clients).await?;

                emit_status_event(
                    app_handle,
                    "Getting RDS endpoint...",
                    Some(connection_id),
                );
                current_rds_endpoint = operations::get_rds_endpoint(
                    clients,
                    &project_config.rds_type,
                    &project_config.rds_pattern,
                )
                .await?
                .ok_or_else(|| {
                    AppError::Aws(
                        "Failed to find the RDS endpoint during reconnection.".to_string(),
                    )
                })?;

                emit_status_event(
                    app_handle,
                    "Reconnecting port forwarding...",
                    Some(connection_id),
                );
            }
            Err(e) => {
                if cancel_token.is_cancelled() {
                    break;
                }

                reconnect_count += 1;
                if reconnect_count > AUTO_RECONNECT_MAX_RETRIES {
                    return Err(e);
                }

                emit_status_event(
                    app_handle,
                    &format!(
                        "Connection error. Retrying... ({}/{})",
                        reconnect_count, AUTO_RECONNECT_MAX_RETRIES
                    ),
                    Some(connection_id),
                );

                tokio::time::sleep(tokio::time::Duration::from_millis(
                    AUTO_RECONNECT_DELAY_MS * 2,
                ))
                .await;

                if cancel_token.is_cancelled() {
                    break;
                }

                // Verify credentials
                let cred_check = operations::check_credentials_valid(clients).await;
                if !cred_check.valid {
                    emit_status_event(
                        app_handle,
                        "AWS credentials expired. Please re-authenticate and reconnect.",
                        Some(connection_id),
                    );
                    break;
                }

                // Re-discover infrastructure (best effort)
                if let Ok(id) = operations::find_bastion_instance(clients).await {
                    current_instance_id = id;
                }
                if let Ok(Some(ep)) = operations::get_rds_endpoint(
                    clients,
                    &project_config.rds_type,
                    &project_config.rds_pattern,
                )
                .await
                {
                    current_rds_endpoint = ep;
                }
            }
        }
    }

    Ok(())
}

/// Start port forwarding with TargetNotConnected retry.
#[allow(clippy::too_many_arguments)]
async fn start_port_forwarding_with_retry(
    clients: &AwsClients,
    instance_id: &str,
    rds_endpoint: &str,
    local_port: &str,
    remote_port: &str,
    region: &str,
    cancel_token: &CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), AppError> {
    let mut current_instance_id = instance_id.to_string();
    let mut retry_count: u32 = 0;
    let mut ready_tx = ready_tx;

    loop {
        let result = execute_port_forwarding(
            clients,
            &current_instance_id,
            rds_endpoint,
            local_port,
            remote_port,
            region,
            cancel_token,
            ready_tx.take(),
        )
        .await;

        match result {
            Ok(()) => return Ok(()),
            Err(PortForwardError::TargetNotConnected) if retry_count < PORT_FORWARDING_MAX_RETRIES => {
                // TargetNotConnected: terminate bastion, wait for ASG replacement
                let _ = operations::terminate_bastion_instance(clients, &current_instance_id).await;

                let new_id = operations::wait_for_new_bastion_instance(
                    clients,
                    &current_instance_id,
                    BASTION_WAIT_MAX_RETRIES,
                    BASTION_WAIT_RETRY_DELAY_MS,
                )
                .await?
                .ok_or_else(|| {
                    AppError::Tunnel(
                        "Failed to find new bastion instance after waiting.".to_string(),
                    )
                })?;

                current_instance_id = new_id;
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

enum PortForwardError {
    TargetNotConnected,
    Cancelled,
    Failed(String),
}

/// Execute a single port forwarding session via native WebSocket (no plugin binary).
#[allow(clippy::too_many_arguments)]
async fn execute_port_forwarding(
    clients: &AwsClients,
    instance_id: &str,
    rds_endpoint: &str,
    local_port: &str,
    remote_port: &str,
    _region: &str,
    cancel_token: &CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) -> Result<(), PortForwardError> {
    // Start SSM session via AWS API
    let session_response = operations::start_session(
        clients,
        instance_id,
        rds_endpoint,
        remote_port,
        local_port,
    )
    .await
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

    // Run native port forwarding (WebSocket + TCP relay)
    let cancel_child = cancel_token.child_token();
    let result = native::start_native_port_forwarding(
        stream_url,
        token_value,
        port_num,
        cancel_child,
        ready_tx,
    )
    .await;

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

fn emit_status_event(app_handle: &AppHandle, message: &str, connection_id: Option<&str>) {
    let mut payload = serde_json::json!({ "message": message });
    if let Some(id) = connection_id {
        payload["connectionId"] = serde_json::json!(id);
    }
    let _ = app_handle.emit("status", &payload);
}
