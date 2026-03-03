use crate::config::projects::{self, ProjectConfig};
use crate::error::AppError;
use crate::sandbox::{self, AwsDirAccess, SandboxStatus};
use crate::tunnel::manager::TunnelManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

pub type AwsDirState = Arc<std::sync::RwLock<Option<AwsDirAccess>>>;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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

#[tauri::command]
pub async fn check_for_updates(app_handle: AppHandle) -> Result<UpdateInfo, AppError> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle
        .updater()
        .map_err(|e| AppError::General(format!("Failed to get updater: {}", e)))?;

    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateInfo {
            update_available: true,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: Some(update.version.clone()),
            download_url: None,
        }),
        Ok(None) => Ok(UpdateInfo {
            update_available: false,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: None,
            download_url: None,
        }),
        Err(e) => {
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
pub async fn install_update(app_handle: AppHandle) -> Result<(), AppError> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle
        .updater()
        .map_err(|e| AppError::General(format!("Failed to get updater: {}", e)))?;

    let update = updater
        .check()
        .await
        .map_err(|e| AppError::General(format!("Failed to check for updates: {}", e)))?
        .ok_or_else(|| AppError::General("No update available".to_string()))?;

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
        .map_err(|e| AppError::General(format!("Failed to download/install update: {}", e)))?;

    app_handle.restart();

    #[allow(unreachable_code)]
    Ok(())
}

#[tauri::command]
pub async fn get_current_version() -> Result<String, AppError> {
    Ok(CURRENT_VERSION.to_string())
}

#[tauri::command]
pub async fn open_url(app_handle: AppHandle, url: String) -> Result<(), AppError> {
    if !url.starts_with("https://") {
        return Err(AppError::General("Only HTTPS URLs are allowed".to_string()));
    }
    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::General(format!("Failed to open URL: {}", e)))
}

#[tauri::command]
pub async fn quit_app(
    app_handle: AppHandle,
    tunnel_manager: State<'_, Arc<Mutex<TunnelManager>>>,
) -> Result<(), AppError> {
    // Disconnect all tunnels
    {
        let manager = tunnel_manager.lock().await;
        let _ = manager.disconnect_all().await;
    }

    // Brief wait for graceful shutdown
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    app_handle.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn get_sandbox_status(app_handle: AppHandle) -> Result<SandboxStatus, AppError> {
    Ok(sandbox::get_sandbox_status(&app_handle))
}

#[tauri::command]
pub async fn grant_aws_access(
    app_handle: AppHandle,
    aws_dir_state: State<'_, AwsDirState>,
) -> Result<(), AppError> {
    // Show folder picker and create bookmark
    let aws_dir_path = sandbox::grant_aws_dir_access(&app_handle).await?;

    // Activate the bookmark (starts security-scoped access, sets env vars)
    let access = sandbox::activate_aws_dir_access(&app_handle)?;

    // Update managed state
    let mut guard = aws_dir_state
        .write()
        .map_err(|_| AppError::General("Failed to acquire state lock".to_string()))?;
    *guard = Some(access);

    log::info!(
        "AWS directory access granted: {}",
        aws_dir_path.display()
    );
    Ok(())
}

#[tauri::command]
pub async fn check_migration_available() -> Result<bool, AppError> {
    // Migration is available if we're sandboxed and have no projects yet
    if !sandbox::is_sandboxed() {
        return Ok(false);
    }
    let configs = projects::load_project_configs().await?;
    Ok(configs.is_empty())
}

#[tauri::command]
pub async fn import_projects_file(app_handle: AppHandle) -> Result<usize, AppError> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app_handle
        .dialog()
        .file()
        .set_title("Select your projects.json file")
        .add_filter("JSON", &["json"])
        .pick_file(move |file_path| {
            let _ = tx.send(file_path);
        });

    let file_path = rx
        .await
        .map_err(|_| AppError::General("Dialog channel closed".to_string()))?
        .ok_or_else(|| AppError::General("File selection cancelled".to_string()))?;

    let path_buf: std::path::PathBuf = file_path
        .as_path()
        .ok_or_else(|| AppError::General("Invalid file path from dialog".to_string()))?
        .to_path_buf();

    let data = tokio::fs::read_to_string(&path_buf)
        .await
        .map_err(|e| AppError::Config(format!("Failed to read file: {}", e)))?;

    let imported: HashMap<String, ProjectConfig> =
        serde_json::from_str(&data).map_err(|e| AppError::Config(format!("Invalid projects.json: {}", e)))?;

    if imported.is_empty() {
        return Ok(0);
    }

    let count = imported.len();
    for (key, config) in imported {
        projects::save_project_config(&key, config).await?;
    }

    Ok(count)
}
