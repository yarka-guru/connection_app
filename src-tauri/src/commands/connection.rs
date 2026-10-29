use crate::aws::sso;
use crate::error::AppError;
use crate::tunnel::manager::{ActiveConnectionInfo, ConnectionInfo, TunnelManager};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConnectResult {
    #[serde(rename = "connectionId")]
    pub connection_id: String,
    #[serde(rename = "connectionInfo")]
    pub connection_info: ConnectionInfo,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect(
    app_handle: AppHandle,
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
    project_key: String,
    environment: String,
    profile: Option<String>,
    local_port: Option<String>,
    database: Option<String>,
    saved_connection_id: Option<String>,
) -> Result<ConnectResult, AppError> {
    let (connection_id, mut connection_info, auth_check) = {
        let manager = tunnel_manager.lock().await;

        // Get currently used ports
        let used_ports = manager.get_used_ports().await;

        manager
            .connect(
                &project_key,
                &environment,
                profile.as_deref(),
                local_port.as_deref(),
                database.as_deref(),
                &used_ports,
            )
            .await?
    };

    // The database sign-in check can take seconds; run it without the
    // manager lock so other commands are not blocked.
    if let Some(check) = auth_check {
        let label = check.run().await.ok_or_else(|| {
            AppError::Tunnel("Disconnected during the database sign-in check".to_string())
        })?;
        connection_info.auth_method = Some(label);
    }

    // Update last used time if this is from a saved connection
    if let Some(saved_id) = saved_connection_id {
        let _ = super::saved::update_saved_connection_last_used_inner(&app_handle, &saved_id);
    }

    // Refresh tray menu to show the new active connection
    crate::tray::refresh_tray(&app_handle);

    Ok(ConnectResult {
        connection_id,
        connection_info,
    })
}

#[tauri::command]
pub async fn disconnect(
    app_handle: AppHandle,
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
    connection_id: Option<String>,
) -> Result<(), AppError> {
    let manager = tunnel_manager.lock().await;

    if let Some(id) = connection_id {
        manager.disconnect(&id).await?;
    } else {
        manager.disconnect_all().await?;
    }

    // Refresh tray menu after disconnecting
    crate::tray::refresh_tray(&app_handle);

    Ok(())
}

#[tauri::command]
pub async fn disconnect_all(
    app_handle: AppHandle,
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
) -> Result<(), AppError> {
    let manager = tunnel_manager.lock().await;
    manager.disconnect_all().await?;

    // Refresh tray menu after disconnecting all
    crate::tray::refresh_tray(&app_handle);

    Ok(())
}

#[tauri::command]
pub async fn get_active_connections_list(
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
) -> Result<Vec<ActiveConnectionInfo>, AppError> {
    let manager = tunnel_manager.lock().await;
    Ok(manager.get_active_connections().await)
}

#[tauri::command]
pub async fn get_used_ports(
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
) -> Result<Vec<String>, AppError> {
    let manager = tunnel_manager.lock().await;
    Ok(manager.get_used_ports().await)
}

#[tauri::command]
pub async fn sso_login(
    app_handle: AppHandle,
    profile: String,
) -> Result<(), AppError> {
    let handler = sso::TauriSsoHandler {
        app_handle: app_handle.clone(),
    };
    sso::ensure_sso_session(&profile, &handler, None).await
}
