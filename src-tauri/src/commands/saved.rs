use crate::error::AppError;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const SAVED_CONNECTIONS_KEY: &str = "savedConnections";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(rename = "lastUsedAt")]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs() * 1000)
}

#[tauri::command]
pub async fn load_saved_connections(app_handle: AppHandle) -> Result<Vec<SavedConnection>, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    Ok(connections)
}

#[tauri::command]
pub async fn save_connection(
    app_handle: AppHandle,
    name: String,
    project_key: String,
    profile: String,
    database: Option<String>,
) -> Result<SavedConnection, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Check if connection with same project/profile/database exists
    if let Some(existing) = connections
        .iter_mut()
        .find(|c| c.project_key == project_key && c.profile == profile && c.database == database)
    {
        existing.name.clone_from(&name);
        existing.last_used_at = Some(chrono_now());
        let saved = existing.clone();

        store.set(
            SAVED_CONNECTIONS_KEY,
            serde_json::to_value(&connections)
                .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
        );
        store
            .save()
            .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

        crate::tray::refresh_tray(&app_handle);

        return Ok(saved);
    }

    let new_connection = SavedConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        project_key,
        profile,
        database,
        last_used_at: Some(chrono_now()),
        group: None,
    };

    connections.push(new_connection.clone());

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    // Refresh tray menu to show updated saved connections
    crate::tray::refresh_tray(&app_handle);

    Ok(new_connection)
}

#[tauri::command]
pub async fn delete_saved_connection(app_handle: AppHandle, id: String) -> Result<(), AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    connections.retain(|c| c.id != id);

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    // Refresh tray menu to show updated saved connections
    crate::tray::refresh_tray(&app_handle);

    Ok(())
}

#[tauri::command]
pub async fn update_saved_connection(
    app_handle: AppHandle,
    id: String,
    name: String,
) -> Result<SavedConnection, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let conn = connections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::General(format!("Connection not found: {}", id)))?;

    conn.name = name;
    let updated = conn.clone();

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(updated)
}

#[tauri::command]
pub async fn reorder_saved_connections(
    app_handle: AppHandle,
    ids: Vec<String>,
) -> Result<(), AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    // Reorder: place connections in the order specified by ids
    let mut reordered: Vec<SavedConnection> = Vec::with_capacity(connections.len());
    for id in &ids {
        if let Some(pos) = connections.iter().position(|c| c.id == *id) {
            reordered.push(connections.remove(pos));
        }
    }
    // Append any connections not in the ids list (shouldn't happen, but safe)
    reordered.append(&mut connections);

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&reordered)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(())
}

#[tauri::command]
pub async fn update_saved_connection_last_used(
    app_handle: AppHandle,
    id: String,
) -> Result<(), AppError> {
    update_saved_connection_last_used_inner(&app_handle, &id)
}

/// Inner function callable from other commands.
pub fn update_saved_connection_last_used_inner(
    app_handle: &AppHandle,
    id: &str,
) -> Result<(), AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    if let Some(conn) = connections.iter_mut().find(|c| c.id == id) {
        conn.last_used_at = Some(chrono_now());
    }

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(())
}

#[tauri::command]
pub async fn move_connection_to_group(
    app_handle: AppHandle,
    id: String,
    group: Option<String>,
) -> Result<SavedConnection, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let conn = connections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::General(format!("Connection not found: {}", id)))?;

    // Normalize empty strings to None
    conn.group = group.filter(|g| !g.trim().is_empty());
    let updated = conn.clone();

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(updated)
}

#[tauri::command]
pub async fn rename_connection_group(
    app_handle: AppHandle,
    old_name: String,
    new_name: String,
) -> Result<Vec<SavedConnection>, AppError> {
    let new_name_trimmed = new_name.trim().to_string();
    if new_name_trimmed.is_empty() {
        return Err(AppError::General("Group name cannot be empty".to_string()));
    }

    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    for conn in &mut connections {
        if conn.group.as_deref() == Some(&old_name) {
            conn.group = Some(new_name_trimmed.clone());
        }
    }

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(connections)
}

#[tauri::command]
pub async fn delete_connection_group(
    app_handle: AppHandle,
    group_name: String,
) -> Result<Vec<SavedConnection>, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;

    let mut connections: Vec<SavedConnection> = store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    for conn in &mut connections {
        if conn.group.as_deref() == Some(&group_name) {
            conn.group = None;
        }
    }

    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(&connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))?;

    Ok(connections)
}
