use crate::commands::projects::load_profile_accounts;
use crate::config::accounts::ProfileAccount;
use crate::config::environments::{ResolvedEnvironment, find_environment, resolve_environments};
use crate::config::preferences::load_preferences;
use crate::config::project_file::load_project_file;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const SAVED_CONNECTIONS_KEY: &str = "savedConnections";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    /// The profile last used; a remembered candidate, not the connection target.
    pub profile: String,
    /// Environment name. `None` for entries saved before environments existed.
    #[serde(default)]
    pub environment: Option<String>,
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

/// A saved connection as the frontend sees it.
#[derive(Serialize, Debug, Clone)]
pub struct SavedConnectionDto {
    #[serde(flatten)]
    pub connection: SavedConnection,
    /// No environment of the project could be found for this entry.
    #[serde(rename = "needsAttention")]
    pub needs_attention: bool,
}

/// The environment a saved profile reaches: the one listing it as a
/// candidate, else the one in the profile's account.
fn environment_for_profile(
    profile: &str,
    envs: &[ResolvedEnvironment],
    accounts: &[ProfileAccount],
) -> Option<String> {
    let by_candidate = envs
        .iter()
        .find(|e| e.candidates.iter().any(|c| c == profile));
    let by_account = || {
        let account = accounts
            .iter()
            .find(|a| a.profile == profile)
            .and_then(|a| a.account_id.as_deref())?;
        envs.iter().find(|e| e.environment.account_id == account)
    };
    by_candidate
        .or_else(by_account)
        .map(|e| e.environment.name.clone())
}

/// Tie every saved connection to an environment of its project.
///
/// An entry whose environment is missing, or no longer exists (a legacy
/// profile-named environment after migration), is matched again by its
/// profile. Entries that match nothing are kept unchanged and need
/// attention. Returns the entries and whether any was changed.
pub(crate) fn link_saved_connections(
    connections: Vec<SavedConnection>,
    envs_by_project: &HashMap<String, Vec<ResolvedEnvironment>>,
    accounts: &[ProfileAccount],
) -> (Vec<SavedConnectionDto>, bool) {
    let mut changed = false;
    let dtos = connections
        .into_iter()
        .map(|mut connection| {
            let envs = envs_by_project
                .get(&connection.project_key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let known = connection
                .environment
                .as_deref()
                .is_some_and(|name| find_environment(envs, name).is_some());
            let needs_attention = if known {
                false
            } else if let Some(name) = environment_for_profile(&connection.profile, envs, accounts)
            {
                connection.environment = Some(name);
                changed = true;
                false
            } else {
                true
            };
            SavedConnectionDto {
                connection,
                needs_attention,
            }
        })
        .collect();
    (dtos, changed)
}

/// Resolved environments of every valid project. Empty when `projects.json`
/// cannot be read, so nothing is converted then.
async fn environments_by_project() -> (
    HashMap<String, Vec<ResolvedEnvironment>>,
    Vec<ProfileAccount>,
) {
    let accounts = load_profile_accounts().await;
    let prefs = load_preferences().await;
    let configs = match load_project_file().await {
        Ok(file) => file.valid_configs(),
        Err(e) => {
            log::warn!("Saved connections not linked to environments: {}", e);
            HashMap::new()
        }
    };
    let envs = configs
        .iter()
        .map(|(key, config)| {
            (
                key.clone(),
                resolve_environments(config, &configs, &accounts, &prefs),
            )
        })
        .collect();
    (envs, accounts)
}

/// The saved connections as stored.
pub(crate) fn read_saved_connections(
    app_handle: &AppHandle,
) -> Result<Vec<SavedConnection>, AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;
    Ok(store
        .get(SAVED_CONNECTIONS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn write_saved_connections(
    app_handle: &AppHandle,
    connections: &[SavedConnection],
) -> Result<(), AppError> {
    let store = app_handle
        .store("connections.json")
        .map_err(|e| AppError::General(format!("Failed to open store: {}", e)))?;
    store.set(
        SAVED_CONNECTIONS_KEY,
        serde_json::to_value(connections)
            .map_err(|e| AppError::General(format!("Serialization error: {}", e)))?,
    );
    store
        .save()
        .map_err(|e| AppError::General(format!("Failed to save store: {}", e)))
}

#[tauri::command]
pub async fn load_saved_connections(
    app_handle: AppHandle,
) -> Result<Vec<SavedConnectionDto>, AppError> {
    let connections = read_saved_connections(&app_handle)?;
    let (envs, accounts) = environments_by_project().await;
    let (dtos, changed) = link_saved_connections(connections, &envs, &accounts);
    if changed {
        let linked: Vec<SavedConnection> = dtos.iter().map(|d| d.connection.clone()).collect();
        write_saved_connections(&app_handle, &linked)?;
    }
    Ok(dtos)
}

#[tauri::command]
pub async fn save_connection(
    app_handle: AppHandle,
    name: String,
    project_key: String,
    environment: String,
    profile: String,
    database: Option<String>,
) -> Result<SavedConnection, AppError> {
    let mut connections = read_saved_connections(&app_handle)?;

    // Check if connection with same project/environment/database exists
    if let Some(existing) = connections.iter_mut().find(|c| {
        c.project_key == project_key
            && c.environment.as_deref() == Some(environment.as_str())
            && c.database == database
    }) {
        existing.name.clone_from(&name);
        existing.profile.clone_from(&profile);
        existing.last_used_at = Some(chrono_now());
        let saved = existing.clone();

        write_saved_connections(&app_handle, &connections)?;
        crate::tray::refresh_tray(&app_handle);

        return Ok(saved);
    }

    let new_connection = SavedConnection {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        project_key,
        profile,
        environment: Some(environment),
        database,
        last_used_at: Some(chrono_now()),
        group: None,
    };

    connections.push(new_connection.clone());
    write_saved_connections(&app_handle, &connections)?;

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

/// Rename a saved connection and, when `environment` is given, point it at
/// that environment.
#[tauri::command]
pub async fn update_saved_connection(
    app_handle: AppHandle,
    id: String,
    name: String,
    environment: Option<String>,
) -> Result<SavedConnection, AppError> {
    let mut connections = read_saved_connections(&app_handle)?;

    let conn = connections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::General(format!("Connection not found: {}", id)))?;

    conn.name = name;
    if let Some(env) = environment.filter(|e| !e.trim().is_empty()) {
        conn.environment = Some(env);
    }
    let updated = conn.clone();

    write_saved_connections(&app_handle, &connections)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::accounts::ProfileAccount;
    use crate::config::environments::ResolvedEnvironment;
    use crate::config::projects::Environment;
    use std::collections::HashMap;

    const PROD: &str = "231030592327";
    const DEV: &str = "073813807522";

    fn resolved(name: &str, account: &str, candidates: &[&str]) -> ResolvedEnvironment {
        ResolvedEnvironment {
            environment: Environment {
                name: name.to_string(),
                account_id: account.to_string(),
                port: "5460".to_string(),
                rds_pattern: None,
                region: None,
                target_pattern: None,
                ecs_cluster: None,
                ecs_service: None,
                bastion_pattern: None,
            },
            legacy: false,
            profile: candidates.first().map(|c| c.to_string()),
            candidates: candidates.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn saved(id: &str, profile: &str, environment: Option<&str>) -> SavedConnection {
        SavedConnection {
            id: id.to_string(),
            name: id.to_string(),
            project_key: "covered".to_string(),
            profile: profile.to_string(),
            environment: environment.map(str::to_string),
            database: None,
            last_used_at: None,
            group: None,
        }
    }

    fn envs() -> HashMap<String, Vec<ResolvedEnvironment>> {
        HashMap::from([(
            "covered".to_string(),
            vec![
                resolved("prod", PROD, &["covered"]),
                resolved("dev", DEV, &[]),
            ],
        )])
    }

    fn accounts() -> Vec<ProfileAccount> {
        vec![
            ProfileAccount {
                profile: "covered".to_string(),
                account_id: Some(PROD.to_string()),
            },
            // Saved before the account had a profile the project matches by name.
            ProfileAccount {
                profile: "old-dev".to_string(),
                account_id: Some(DEV.to_string()),
            },
        ]
    }

    #[test]
    fn saved_connection_maps_profile_to_environment() {
        let conns = vec![saved("a", "covered", None), saved("b", "old-dev", None)];
        let (dtos, changed) = link_saved_connections(conns, &envs(), &accounts());
        assert!(changed);
        // By candidate, then by the profile's account.
        assert_eq!(dtos[0].connection.environment.as_deref(), Some("prod"));
        assert_eq!(dtos[1].connection.environment.as_deref(), Some("dev"));
        assert!(dtos.iter().all(|d| !d.needs_attention));
        // The profile stays as the remembered candidate.
        assert_eq!(dtos[1].connection.profile, "old-dev");
    }

    #[test]
    fn saved_connection_without_match_needs_attention_and_is_kept() {
        let mut other_project = saved("c", "covered", None);
        other_project.project_key = "gone".to_string();
        let conns = vec![saved("a", "unknown-profile", None), other_project];
        let (dtos, changed) = link_saved_connections(conns, &envs(), &accounts());
        assert!(!changed);
        assert_eq!(dtos.len(), 2);
        assert!(dtos.iter().all(|d| d.needs_attention));
        assert!(dtos.iter().all(|d| d.connection.environment.is_none()));
        assert_eq!(dtos[0].connection.profile, "unknown-profile");
    }

    #[test]
    fn saved_connection_with_a_known_environment_is_unchanged() {
        let conns = vec![saved("a", "someone-else", Some("dev"))];
        let (dtos, changed) = link_saved_connections(conns, &envs(), &accounts());
        assert!(!changed);
        assert!(!dtos[0].needs_attention);
        assert_eq!(dtos[0].connection.environment.as_deref(), Some("dev"));
    }

    #[test]
    fn saved_connection_to_a_renamed_environment_is_relinked_by_profile() {
        // Saved against the legacy, profile-named environment before migration.
        let conns = vec![saved("a", "covered", Some("covered"))];
        let (dtos, changed) = link_saved_connections(conns, &envs(), &accounts());
        assert!(changed);
        assert_eq!(dtos[0].connection.environment.as_deref(), Some("prod"));
        assert!(!dtos[0].needs_attention);
    }

    #[test]
    fn saved_connection_dto_is_flat_camel_case() {
        let dto = SavedConnectionDto {
            connection: saved("a", "covered", Some("prod")),
            needs_attention: true,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["environment"], "prod");
        assert_eq!(v["projectKey"], "covered");
        assert_eq!(v["needsAttention"], true);
    }
}
