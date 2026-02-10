use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;
use tauri_plugin_store::StoreExt;
use tokio::sync::{oneshot, Mutex as TokioMutex};
use uuid::Uuid;

// Per-command response map: command ID -> oneshot sender
type PendingResponses = Arc<TokioMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

// State management - use tokio Mutex for async safety
struct SidecarState {
    child: Option<CommandChild>,
    pending: PendingResponses,
}

impl Default for SidecarState {
    fn default() -> Self {
        Self {
            child: None,
            pending: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }
}

// Command ID counter
static COMMAND_ID: AtomicU64 = AtomicU64::new(1);

// Response types
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Project {
    pub key: String,
    pub name: String,
}

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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConnectResult {
    #[serde(rename = "connectionId")]
    pub connection_id: String,
    #[serde(rename = "connectionInfo")]
    pub connection_info: ConnectionInfo,
}

// Saved connection type
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    pub profile: String,
    #[serde(rename = "lastUsedAt")]
    pub last_used_at: Option<String>,
}

// Active connection tracking
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ActiveConnection {
    pub id: String,
    #[serde(rename = "savedConnectionId")]
    pub saved_connection_id: Option<String>,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    pub profile: String,
    #[serde(rename = "localPort")]
    pub local_port: String,
    #[serde(rename = "connectionInfo")]
    pub connection_info: ConnectionInfo,
    pub status: String,
}

// Update info type
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateInfo {
    #[serde(rename = "updateAvailable")]
    pub update_available: bool,
    #[serde(rename = "currentVersion")]
    pub current_version: String,
    #[serde(rename = "latestVersion")]
    pub latest_version: Option<String>,
    #[serde(rename = "downloadUrl")]
    pub download_url: Option<String>,
}

// Active connections map - tracks all currently active connections
static ACTIVE_CONNECTIONS: std::sync::OnceLock<Arc<TokioMutex<HashMap<String, ActiveConnection>>>> =
    std::sync::OnceLock::new();

fn get_active_connections() -> Arc<TokioMutex<HashMap<String, ActiveConnection>>> {
    ACTIVE_CONNECTIONS
        .get_or_init(|| Arc::new(TokioMutex::new(HashMap::new())))
        .clone()
}

// Initialize sidecar if not already running
async fn ensure_sidecar(
    app_handle: &AppHandle,
    state: &Arc<TokioMutex<SidecarState>>,
) -> Result<(), String> {
    let mut state_guard = state.lock().await;

    // Return if sidecar is already running
    if state_guard.child.is_some() {
        return Ok(());
    }

    let pending = state_guard.pending.clone();

    // Spawn sidecar using Tauri shell plugin
    // Pass through all environment variables to ensure aws-vault and AWS CLI are accessible
    let mut sidecar_command = app_handle
        .shell()
        .sidecar("gui-adapter")
        .map_err(|e| format!("Failed to create sidecar command: {}", e))?;

    // Get current PATH and extend with common installation locations
    let current_path = std::env::var("PATH").unwrap_or_default();
    let extended_path = format!(
        "{}:/usr/local/bin:/opt/homebrew/bin:/home/linuxbrew/.linuxbrew/bin:{}/.local/bin",
        current_path,
        std::env::var("HOME").unwrap_or_default()
    );
    sidecar_command = sidecar_command.env("PATH", extended_path);

    // Pass through important environment variables
    // Exclude AWS_VAULT to avoid "running in existing subshell" error
    for (key, value) in std::env::vars() {
        if (key.starts_with("AWS_") && key != "AWS_VAULT") || key == "HOME" || key == "USER" || key == "SHELL" {
            sidecar_command = sidecar_command.env(&key, value);
        }
    }
    // Explicitly clear AWS_VAULT to ensure clean environment
    sidecar_command = sidecar_command.env("AWS_VAULT", "");

    let (mut event_rx, child) = sidecar_command
        .spawn()
        .map_err(|e| format!("Failed to spawn sidecar: {}", e))?;

    state_guard.child = Some(child);

    // Drop the lock before spawning async task
    drop(state_guard);

    // Clone Arc for the reader task (no unsafe pointer needed)
    let state_clone = Arc::clone(state);
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    // Spawn task to read stdout, route responses, and forward events
    let app_handle_clone = app_handle.clone();
    let pending_clone = pending.clone();
    tokio::spawn(async move {
        use tauri_plugin_shell::process::CommandEvent;

        let mut ready_tx = Some(ready_tx);

        while let Some(event) = event_rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    let line_str = String::from_utf8_lossy(&line);
                    for json_str in line_str.lines() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                            // Signal readiness when sidecar sends ready event
                            if json.get("event") == Some(&serde_json::json!("ready")) {
                                if let Some(tx) = ready_tx.take() {
                                    let _ = tx.send(());
                                }
                            }

                            // Route responses to the matching per-command oneshot
                            if let Some(id) = json.get("id").and_then(|v| v.as_u64()) {
                                let mut map = pending_clone.lock().await;
                                if let Some(tx) = map.remove(&id) {
                                    let _ = tx.send(json.clone());
                                }
                            }
                            // Forward all messages (responses + events) to the frontend
                            let _ = app_handle_clone.emit("sidecar-event", json);
                        }
                    }
                }
                CommandEvent::Stderr(line) => {
                    eprintln!("Sidecar stderr: {}", String::from_utf8_lossy(&line));
                }
                CommandEvent::Terminated(status) => {
                    eprintln!("Sidecar terminated with status: {:?}", status);

                    // Clear sidecar child so ensure_sidecar can restart it
                    let mut guard = state_clone.lock().await;
                    guard.child = None;

                    // Fail all pending commands
                    let mut map = pending_clone.lock().await;
                    for (_id, tx) in map.drain() {
                        let _ = tx.send(serde_json::json!({
                            "type": "error",
                            "message": "Sidecar terminated unexpectedly"
                        }));
                    }

                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for sidecar ready event instead of arbitrary sleep
    match tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        ready_rx,
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err("Sidecar process exited before becoming ready".to_string()),
        Err(_) => Err("Sidecar failed to start within 10 seconds".to_string()),
    }
}

// Send command to sidecar and wait for response
async fn send_command_and_wait(
    app_handle: &AppHandle,
    state: &Arc<TokioMutex<SidecarState>>,
    action: &str,
    params: serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, String> {
    ensure_sidecar(app_handle, state).await?;

    let id = COMMAND_ID.fetch_add(1, Ordering::SeqCst);

    // Build command
    let mut command = serde_json::json!({
        "id": id,
        "action": action
    });
    if let serde_json::Value::Object(map) = params {
        for (k, v) in map {
            command[k] = v;
        }
    }

    // Create a oneshot channel for this command's response
    let (resp_tx, resp_rx) = oneshot::channel::<serde_json::Value>();

    // Register the sender in the pending map
    let pending = {
        let guard = state.lock().await;
        guard.pending.clone()
    };
    {
        let mut map = pending.lock().await;
        map.insert(id, resp_tx);
    }

    // Send command to sidecar stdin
    {
        let mut state_guard = state.lock().await;
        if let Some(ref mut child) = state_guard.child {
            let command_str = serde_json::to_string(&command).map_err(|e| e.to_string())?;
            child
                .write(format!("{}\n", command_str).as_bytes())
                .map_err(|e| {
                    // Clean up pending entry on write failure
                    let pending = state_guard.pending.clone();
                    tokio::spawn(async move {
                        pending.lock().await.remove(&id);
                    });
                    format!("Failed to write to sidecar: {}", e)
                })?;
        } else {
            pending.lock().await.remove(&id);
            return Err("Sidecar not running".to_string());
        }
    }

    // Await the oneshot with timeout
    let timeout = tokio::time::Duration::from_millis(timeout_ms);
    match tokio::time::timeout(timeout, resp_rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err("Response channel dropped (sidecar may have crashed)".to_string()),
        Err(_) => {
            // Timeout — clean up the pending entry
            pending.lock().await.remove(&id);
            Err("Timeout waiting for sidecar response".to_string())
        }
    }
}

// ========================
// SAVED CONNECTIONS COMMANDS
// ========================

const SAVED_CONNECTIONS_KEY: &str = "savedConnections";

#[tauri::command]
async fn load_saved_connections(app_handle: AppHandle) -> Result<Vec<SavedConnection>, String> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| format!("Failed to open store: {}", e))?;

    let connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    Ok(connections)
}

#[tauri::command]
async fn save_connection(
    app_handle: AppHandle,
    name: String,
    project_key: String,
    profile: String,
) -> Result<SavedConnection, String> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| format!("Failed to open store: {}", e))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Check if connection with same project/profile exists
    if let Some(existing) = connections.iter_mut().find(|c| c.project_key == project_key && c.profile == profile) {
        existing.name = name.clone();
        existing.last_used_at = Some(chrono_now());
        let saved = existing.clone();

        store.set(SAVED_CONNECTIONS_KEY, serde_json::to_value(&connections).map_err(|e| format!("Serialization error: {}", e))?);
        store.save().map_err(|e| format!("Failed to save store: {}", e))?;

        return Ok(saved);
    }

    let new_connection = SavedConnection {
        id: Uuid::new_v4().to_string(),
        name,
        project_key,
        profile,
        last_used_at: Some(chrono_now()),
    };

    connections.push(new_connection.clone());

    store.set(SAVED_CONNECTIONS_KEY, serde_json::to_value(&connections).map_err(|e| format!("Serialization error: {}", e))?);
    store.save().map_err(|e| format!("Failed to save store: {}", e))?;

    Ok(new_connection)
}

#[tauri::command]
async fn delete_saved_connection(app_handle: AppHandle, id: String) -> Result<(), String> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| format!("Failed to open store: {}", e))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    connections.retain(|c| c.id != id);

    store.set(SAVED_CONNECTIONS_KEY, serde_json::to_value(&connections).map_err(|e| format!("Serialization error: {}", e))?);
    store.save().map_err(|e| format!("Failed to save store: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn update_saved_connection_last_used(app_handle: AppHandle, id: String) -> Result<(), String> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| format!("Failed to open store: {}", e))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    if let Some(conn) = connections.iter_mut().find(|c| c.id == id) {
        conn.last_used_at = Some(chrono_now());
    }

    store.set(SAVED_CONNECTIONS_KEY, serde_json::to_value(&connections).map_err(|e| format!("Serialization error: {}", e))?);
    store.save().map_err(|e| format!("Failed to save store: {}", e))?;

    Ok(())
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Return Unix epoch in milliseconds (for JS compatibility)
    let secs = duration.as_secs();
    format!("{}", secs * 1000)
}

// ========================
// CONNECTION COMMANDS
// ========================

#[tauri::command]
async fn list_projects(
    app_handle: AppHandle,
    state: tauri::State<'_, Arc<TokioMutex<SidecarState>>>,
) -> Result<Vec<Project>, String> {
    let response =
        send_command_and_wait(&app_handle, &state, "list-projects", serde_json::json!({}), 30000)
            .await?;

    if response.get("type") == Some(&serde_json::json!("error")) {
        return Err(response["message"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string());
    }

    let projects: Vec<Project> = serde_json::from_value(
        response
            .get("projects")
            .cloned()
            .unwrap_or(serde_json::json!([])),
    )
    .map_err(|e| e.to_string())?;

    Ok(projects)
}

#[tauri::command]
async fn list_profiles(
    app_handle: AppHandle,
    state: tauri::State<'_, Arc<TokioMutex<SidecarState>>>,
    project_key: String,
) -> Result<Vec<String>, String> {
    let response = send_command_and_wait(
        &app_handle,
        &state,
        "list-profiles",
        serde_json::json!({ "projectKey": project_key }),
        30000,
    )
    .await?;

    if response.get("type") == Some(&serde_json::json!("error")) {
        return Err(response["message"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string());
    }

    let profiles: Vec<String> = serde_json::from_value(
        response
            .get("profiles")
            .cloned()
            .unwrap_or(serde_json::json!([])),
    )
    .map_err(|e| e.to_string())?;

    Ok(profiles)
}

#[tauri::command]
async fn get_used_ports() -> Result<Vec<String>, String> {
    let connections = get_active_connections();
    let guard = connections.lock().await;
    let ports: Vec<String> = guard.values().map(|c| c.local_port.clone()).collect();
    Ok(ports)
}

#[tauri::command]
async fn connect(
    app_handle: AppHandle,
    state: tauri::State<'_, Arc<TokioMutex<SidecarState>>>,
    project_key: String,
    profile: String,
    local_port: Option<String>,
    saved_connection_id: Option<String>,
) -> Result<ConnectResult, String> {
    // Get currently used ports
    let connections = get_active_connections();
    let used_ports: Vec<String> = {
        let guard = connections.lock().await;
        guard.values().map(|c| c.local_port.clone()).collect()
    };

    let response = send_command_and_wait(
        &app_handle,
        &state,
        "connect",
        serde_json::json!({
            "projectKey": project_key,
            "profile": profile,
            "localPort": local_port,
            "usedPorts": used_ports
        }),
        120000,
    )
    .await?;

    if response.get("type") == Some(&serde_json::json!("error")) {
        return Err(response["message"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string());
    }

    let connection_info: ConnectionInfo = serde_json::from_value(
        response
            .get("connectionInfo")
            .cloned()
            .ok_or_else(|| "No connection info in response".to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let connection_id = response
        .get("connectionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Track the active connection
    let active_connection = ActiveConnection {
        id: connection_id.clone(),
        saved_connection_id: saved_connection_id.clone(),
        project_key: project_key.clone(),
        profile: profile.clone(),
        local_port: connection_info.port.clone(),
        connection_info: connection_info.clone(),
        status: "connected".to_string(),
    };

    {
        let mut guard = connections.lock().await;
        guard.insert(connection_id.clone(), active_connection);
    }

    // Update last used time if this is from a saved connection
    if let Some(saved_id) = saved_connection_id {
        let _ = update_saved_connection_last_used(app_handle, saved_id).await;
    }

    Ok(ConnectResult { connection_id, connection_info })
}

#[tauri::command]
async fn disconnect(
    app_handle: AppHandle,
    state: tauri::State<'_, Arc<TokioMutex<SidecarState>>>,
    connection_id: Option<String>,
) -> Result<(), String> {
    let connections = get_active_connections();

    let response = send_command_and_wait(
        &app_handle,
        &state,
        "disconnect",
        serde_json::json!({ "connectionId": connection_id }),
        5000,
    )
    .await?;

    if response.get("type") == Some(&serde_json::json!("error")) {
        return Err(response["message"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string());
    }

    // Remove from active connections
    {
        let mut guard = connections.lock().await;
        if let Some(id) = connection_id {
            guard.remove(&id);
        } else {
            guard.clear();
        }
    }

    Ok(())
}

#[tauri::command]
async fn disconnect_all(
    app_handle: AppHandle,
    state: tauri::State<'_, Arc<TokioMutex<SidecarState>>>,
) -> Result<(), String> {
    let connections = get_active_connections();

    let response = send_command_and_wait(
        &app_handle,
        &state,
        "disconnect-all",
        serde_json::json!({}),
        5000,
    )
    .await?;

    if response.get("type") == Some(&serde_json::json!("error")) {
        return Err(response["message"]
            .as_str()
            .unwrap_or("Unknown error")
            .to_string());
    }

    // Clear all active connections
    {
        let mut guard = connections.lock().await;
        guard.clear();
    }

    Ok(())
}

#[tauri::command]
async fn get_active_connections_list() -> Result<Vec<ActiveConnection>, String> {
    let connections = get_active_connections();
    let guard = connections.lock().await;
    Ok(guard.values().cloned().collect())
}

// ========================
// UPDATE COMMANDS
// ========================

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tauri::command]
async fn check_for_updates(app_handle: AppHandle) -> Result<UpdateInfo, String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle.updater().map_err(|e| format!("Failed to get updater: {}", e))?;

    match updater.check().await {
        Ok(Some(update)) => {
            Ok(UpdateInfo {
                update_available: true,
                current_version: CURRENT_VERSION.to_string(),
                latest_version: Some(update.version.clone()),
                download_url: None, // Not needed for auto-update
            })
        }
        Ok(None) => {
            Ok(UpdateInfo {
                update_available: false,
                current_version: CURRENT_VERSION.to_string(),
                latest_version: None,
                download_url: None,
            })
        }
        Err(e) => {
            // Return no update available if check fails
            eprintln!("Update check failed: {}", e);
            Ok(UpdateInfo {
                update_available: false,
                current_version: CURRENT_VERSION.to_string(),
                latest_version: None,
                download_url: None,
            })
        }
    }
}

#[tauri::command]
async fn install_update(app_handle: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle.updater().map_err(|e| format!("Failed to get updater: {}", e))?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("Failed to check for updates: {}", e))?
        .ok_or_else(|| "No update available".to_string())?;

    // Download and install the update
    let mut downloaded = 0;
    update
        .download_and_install(
            |chunk_length, content_length| {
                downloaded += chunk_length;
                if let Some(total) = content_length {
                    eprintln!("Downloaded {} of {} bytes", downloaded, total);
                }
            },
            || {
                eprintln!("Download complete, installing...");
            },
        )
        .await
        .map_err(|e| format!("Failed to download/install update: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn get_current_version() -> Result<String, String> {
    Ok(CURRENT_VERSION.to_string())
}

#[tauri::command]
async fn open_url(app_handle: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Failed to open URL: {}", e))
}

// ========================
// PREREQUISITES CHECK
// ========================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrerequisiteStatus {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    #[serde(rename = "installUrl")]
    pub install_url: String,
    #[serde(rename = "installCommand")]
    pub install_command: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrerequisitesResult {
    #[serde(rename = "allInstalled")]
    pub all_installed: bool,
    pub prerequisites: Vec<PrerequisiteStatus>,
}

#[tauri::command]
async fn check_prerequisites() -> Result<PrerequisitesResult, String> {
    // Run all blocking I/O on a dedicated thread to avoid starving the Tokio runtime
    tokio::task::spawn_blocking(move || {
        use std::process::Command;

        // Common paths where tools might be installed
        let home = std::env::var("HOME").unwrap_or_default();
        let local_bin = format!("{}/.local/bin", home);
        let search_paths: Vec<&str> = vec![
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/usr/bin",
            "/bin",
            &local_bin,
        ];

        fn find_command(name: &str, search_paths: &[&str]) -> Option<std::path::PathBuf> {
            // First try PATH
            if let Ok(output) = std::process::Command::new("which").arg(name).output()
                && output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path.is_empty() {
                        return Some(std::path::PathBuf::from(path));
                    }
                }
            // Then search common paths
            for dir in search_paths {
                let path = std::path::Path::new(dir).join(name);
                if path.exists() {
                    return Some(path);
                }
            }
            None
        }

        // OS-agnostic install command helper
        fn install_command_for(macos_cmd: &str, _linux_hint: &str) -> Option<String> {
            if cfg!(target_os = "macos") {
                Some(macos_cmd.to_string())
            } else if cfg!(target_os = "linux") {
                Some(_linux_hint.to_string())
            } else if cfg!(target_os = "windows") {
                None // Windows users should follow the install URL
            } else {
                None
            }
        }

        let mut prerequisites = Vec::new();

        // Check aws-vault
        let aws_vault = match find_command("aws-vault", &search_paths) {
            Some(path) => match Command::new(&path).arg("--version").output() {
                Ok(output) if output.status.success() => PrerequisiteStatus {
                    name: "aws-vault".to_string(),
                    installed: true,
                    version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
                    install_url: "https://github.com/99designs/aws-vault#installing".to_string(),
                    install_command: install_command_for("brew install aws-vault", "See install URL for Linux packages"),
                },
                _ => PrerequisiteStatus {
                    name: "aws-vault".to_string(),
                    installed: false,
                    version: None,
                    install_url: "https://github.com/99designs/aws-vault#installing".to_string(),
                    install_command: install_command_for("brew install aws-vault", "See install URL for Linux packages"),
                },
            },
            None => PrerequisiteStatus {
                name: "aws-vault".to_string(),
                installed: false,
                version: None,
                install_url: "https://github.com/99designs/aws-vault#installing".to_string(),
                install_command: install_command_for("brew install aws-vault", "See install URL for Linux packages"),
            },
        };
        prerequisites.push(aws_vault);

        // Check AWS CLI
        let aws_cli = match find_command("aws", &search_paths) {
            Some(path) => match Command::new(&path).arg("--version").output() {
                Ok(output) if output.status.success() => {
                    let version_str = String::from_utf8_lossy(&output.stdout);
                    PrerequisiteStatus {
                        name: "AWS CLI".to_string(),
                        installed: true,
                        version: Some(version_str.split_whitespace().take(1).collect()),
                        install_url: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html".to_string(),
                        install_command: install_command_for("brew install awscli", "curl \"https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip\" -o awscliv2.zip && unzip awscliv2.zip && sudo ./aws/install"),
                    }
                }
                _ => PrerequisiteStatus {
                    name: "AWS CLI".to_string(),
                    installed: false,
                    version: None,
                    install_url: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html".to_string(),
                    install_command: install_command_for("brew install awscli", "curl \"https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip\" -o awscliv2.zip && unzip awscliv2.zip && sudo ./aws/install"),
                },
            },
            None => PrerequisiteStatus {
                name: "AWS CLI".to_string(),
                installed: false,
                version: None,
                install_url: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html".to_string(),
                install_command: install_command_for("brew install awscli", "curl \"https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip\" -o awscliv2.zip && unzip awscliv2.zip && sudo ./aws/install"),
            },
        };
        prerequisites.push(aws_cli);

        // Check Session Manager Plugin
        let ssm_plugin = match find_command("session-manager-plugin", &search_paths) {
            Some(path) => match Command::new(&path).arg("--version").output() {
                Ok(output) if output.status.success() => PrerequisiteStatus {
                    name: "Session Manager Plugin".to_string(),
                    installed: true,
                    version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
                    install_url: "https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html".to_string(),
                    install_command: None,
                },
                _ => PrerequisiteStatus {
                    name: "Session Manager Plugin".to_string(),
                    installed: false,
                    version: None,
                    install_url: "https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html".to_string(),
                    install_command: None,
                },
            },
            None => PrerequisiteStatus {
                name: "Session Manager Plugin".to_string(),
                installed: false,
                version: None,
                install_url: "https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html".to_string(),
                install_command: None,
            },
        };
        prerequisites.push(ssm_plugin);

        let all_installed = prerequisites.iter().all(|p| p.installed);

        Ok(PrerequisitesResult {
            all_installed,
            prerequisites,
        })
    })
    .await
    .map_err(|e| format!("Prerequisites check task failed: {}", e))?
}

// ========================
// AWS CONFIG MANAGEMENT
// ========================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AwsProfile {
    pub name: String,
    pub region: Option<String>,
    #[serde(rename = "sourceProfile")]
    pub source_profile: Option<String>,
    #[serde(rename = "roleArn")]
    pub role_arn: Option<String>,
    #[serde(rename = "mfaSerial")]
    pub mfa_serial: Option<String>,
    #[serde(rename = "ssoStartUrl")]
    pub sso_start_url: Option<String>,
    #[serde(rename = "ssoRegion")]
    pub sso_region: Option<String>,
    #[serde(rename = "ssoAccountId")]
    pub sso_account_id: Option<String>,
    #[serde(rename = "ssoRoleName")]
    pub sso_role_name: Option<String>,
    #[serde(rename = "rawContent")]
    pub raw_content: String,
}

fn get_aws_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home).join(".aws").join("config")
}

#[tauri::command]
async fn read_aws_config() -> Result<Vec<AwsProfile>, String> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(Vec::new());
    }

    let content = tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| format!("Failed to read AWS config: {}", e))?;

    let mut profiles = Vec::new();
    let mut current_profile: Option<String> = None;
    let mut current_content = String::new();
    let mut current_region = None;
    let mut current_source_profile = None;
    let mut current_role_arn = None;
    let mut current_mfa_serial = None;
    let mut current_sso_start_url = None;
    let mut current_sso_region = None;
    let mut current_sso_account_id = None;
    let mut current_sso_role_name = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // Save previous profile
            if let Some(name) = current_profile.take() {
                profiles.push(AwsProfile {
                    name,
                    region: current_region.take(),
                    source_profile: current_source_profile.take(),
                    role_arn: current_role_arn.take(),
                    mfa_serial: current_mfa_serial.take(),
                    sso_start_url: current_sso_start_url.take(),
                    sso_region: current_sso_region.take(),
                    sso_account_id: current_sso_account_id.take(),
                    sso_role_name: current_sso_role_name.take(),
                    raw_content: current_content.trim().to_string(),
                });
                current_content = String::new();
            }

            // Parse new profile name
            let section = &trimmed[1..trimmed.len()-1];
            let profile_name = if let Some(stripped) = section.strip_prefix("profile ") {
                stripped.to_string()
            } else if section == "default" {
                "default".to_string()
            } else {
                section.to_string()
            };
            current_profile = Some(profile_name);
        } else if current_profile.is_some() && !trimmed.is_empty() {
            // Include comment lines (# and ;) in raw_content to preserve them on save
            current_content.push_str(line);
            current_content.push('\n');

            // Only parse key=value lines (skip comments)
            if !trimmed.starts_with('#') && !trimmed.starts_with(';') {
                if let Some((key, value)) = trimmed.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "region" => current_region = Some(value.to_string()),
                        "source_profile" => current_source_profile = Some(value.to_string()),
                        "role_arn" => current_role_arn = Some(value.to_string()),
                        "mfa_serial" => current_mfa_serial = Some(value.to_string()),
                        "sso_start_url" => current_sso_start_url = Some(value.to_string()),
                        "sso_region" => current_sso_region = Some(value.to_string()),
                        "sso_account_id" => current_sso_account_id = Some(value.to_string()),
                        "sso_role_name" => current_sso_role_name = Some(value.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }

    // Save last profile
    if let Some(name) = current_profile {
        profiles.push(AwsProfile {
            name,
            region: current_region,
            source_profile: current_source_profile,
            role_arn: current_role_arn,
            mfa_serial: current_mfa_serial,
            sso_start_url: current_sso_start_url,
            sso_region: current_sso_region,
            sso_account_id: current_sso_account_id,
            sso_role_name: current_sso_role_name,
            raw_content: current_content.trim().to_string(),
        });
    }

    Ok(profiles)
}

/// Match a section name against a profile name, handling both
/// `[profile name]` and bare `[name]` formats.
fn section_matches_profile(section: &str, profile_name: &str) -> bool {
    if profile_name == "default" {
        section == "default"
    } else {
        section == format!("profile {}", profile_name) || section == profile_name
    }
}

#[tauri::command]
async fn save_aws_profile(profile: AwsProfile) -> Result<(), String> {
    let config_path = get_aws_config_path();

    // Ensure .aws directory exists
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create .aws directory: {}", e))?;
    }

    // Read existing config
    let existing_content = if tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        tokio::fs::read_to_string(&config_path)
            .await
            .map_err(|e| format!("Failed to read AWS config: {}", e))?
    } else {
        String::new()
    };

    // Parse and rebuild config
    let mut new_content = String::new();
    let mut found = false;
    let mut in_target_profile = false;
    let profile_header = if profile.name == "default" {
        "[default]".to_string()
    } else {
        format!("[profile {}]", profile.name)
    };

    for line in existing_content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_target_profile {
                in_target_profile = false;
            }

            let section = &trimmed[1..trimmed.len()-1];
            if section_matches_profile(section, &profile.name) {
                in_target_profile = true;
                found = true;
                new_content.push_str(&profile_header);
                new_content.push('\n');
                new_content.push_str(&profile.raw_content);
                new_content.push('\n');
                continue;
            }
        }

        if !in_target_profile {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    // Add new profile if not found
    if !found {
        if !new_content.is_empty() && !new_content.ends_with("\n\n") {
            new_content.push('\n');
        }
        new_content.push_str(&profile_header);
        new_content.push('\n');
        new_content.push_str(&profile.raw_content);
        new_content.push('\n');
    }

    tokio::fs::write(&config_path, new_content.trim_end())
        .await
        .map_err(|e| format!("Failed to write AWS config: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn delete_aws_profile(profile_name: String) -> Result<(), String> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(());
    }

    let content = tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| format!("Failed to read AWS config: {}", e))?;

    let mut new_content = String::new();
    let mut in_target_profile = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len()-1];
            in_target_profile = section_matches_profile(section, &profile_name);

            if !in_target_profile {
                new_content.push_str(line);
                new_content.push('\n');
            }
        } else if !in_target_profile {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    tokio::fs::write(&config_path, new_content.trim_end())
        .await
        .map_err(|e| format!("Failed to write AWS config: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn get_raw_aws_config() -> Result<String, String> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(String::new());
    }

    tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| format!("Failed to read AWS config: {}", e))
}

#[tauri::command]
async fn save_raw_aws_config(content: String) -> Result<(), String> {
    let config_path = get_aws_config_path();

    // Ensure .aws directory exists
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create .aws directory: {}", e))?;
    }

    tokio::fs::write(&config_path, content)
        .await
        .map_err(|e| format!("Failed to write AWS config: {}", e))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::default().build())
        .manage(Arc::new(TokioMutex::new(SidecarState::default())))
        .invoke_handler(tauri::generate_handler![
            // Connection commands
            list_projects,
            list_profiles,
            connect,
            disconnect,
            disconnect_all,
            get_active_connections_list,
            get_used_ports,
            // Saved connections commands
            load_saved_connections,
            save_connection,
            delete_saved_connection,
            update_saved_connection_last_used,
            // Update commands
            check_for_updates,
            install_update,
            get_current_version,
            open_url,
            // Prerequisites commands
            check_prerequisites,
            // AWS config commands
            read_aws_config,
            save_aws_profile,
            delete_aws_profile,
            get_raw_aws_config,
            save_raw_aws_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
