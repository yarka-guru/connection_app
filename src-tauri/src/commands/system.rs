use crate::config::projects::{self, ProjectConfig};
use crate::error::AppError;
use crate::sandbox::{self, AwsDirAccess, SandboxStatus};
use crate::tunnel::manager::TunnelManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

pub type AwsDirState = Arc<std::sync::RwLock<Option<AwsDirAccess>>>;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "https://github.com/yarka-guru/connection_app";

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
    #[serde(rename = "installMethod")]
    pub install_method: String,
}

/// How the app was installed on Linux. Determines update strategy.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxInstallMethod {
    AppImage,
    Deb,
    Homebrew,
    Unknown,
}

#[cfg(target_os = "linux")]
impl LinuxInstallMethod {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AppImage => "appimage",
            Self::Deb => "deb",
            Self::Homebrew => "homebrew",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_install_method() -> LinuxInstallMethod {
    // 1. AppImage: APPIMAGE env var is set, or exe path ends with .AppImage
    if std::env::var("APPIMAGE").is_ok() {
        return LinuxInstallMethod::AppImage;
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe_str = exe.to_string_lossy();
        if exe_str.ends_with(".AppImage") {
            return LinuxInstallMethod::AppImage;
        }
        // 2. Homebrew: exe path contains linuxbrew or homebrew
        if exe_str.contains("linuxbrew") || exe_str.contains("homebrew") || exe_str.contains("Cellar") {
            return LinuxInstallMethod::Homebrew;
        }
        // 3. Deb: dpkg -S <exe_path> succeeds
        if let Ok(status) = std::process::Command::new("dpkg")
            .args(["-S", &exe_str])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return LinuxInstallMethod::Deb;
            }
        }
    }
    LinuxInstallMethod::Unknown
}

fn get_install_method() -> String {
    #[cfg(target_os = "linux")]
    {
        detect_linux_install_method().as_str().to_string()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "native".to_string()
    }
}

#[tauri::command]
pub async fn check_for_updates(app_handle: AppHandle) -> Result<UpdateInfo, AppError> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle
        .updater()
        .map_err(|e| AppError::General(format!("Failed to get updater: {}", e)))?;

    let install_method = get_install_method();

    match updater.check().await {
        Ok(Some(update)) => {
            let download_url = format!("{}/releases/tag/v{}", GITHUB_REPO, update.version);
            Ok(UpdateInfo {
                update_available: true,
                current_version: CURRENT_VERSION.to_string(),
                latest_version: Some(update.version.clone()),
                download_url: Some(download_url),
                install_method,
            })
        }
        Ok(None) => Ok(UpdateInfo {
            update_available: false,
            current_version: CURRENT_VERSION.to_string(),
            latest_version: None,
            download_url: None,
            install_method,
        }),
        Err(e) => {
            eprintln!("Update check failed: {}", e);
            Ok(UpdateInfo {
                update_available: false,
                current_version: CURRENT_VERSION.to_string(),
                latest_version: None,
                download_url: None,
                install_method,
            })
        }
    }
}

/// Download a .deb from GitHub releases and install via pkexec dpkg -i.
#[cfg(target_os = "linux")]
async fn install_deb_update(version: &str, app_handle: &AppHandle) -> Result<(), AppError> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "amd64",
    };
    let url = format!(
        "{}/releases/download/v{}/RDS.SSM.Connect_{}_{}.deb",
        GITHUB_REPO, version, version, arch
    );

    let _ = app_handle.emit(
        "update-progress",
        serde_json::json!({ "phase": "downloading" }),
    );

    let response = reqwest::get(&url)
        .await
        .map_err(|e| AppError::General(format!("Failed to download .deb: {}", e)))?;

    if !response.status().is_success() {
        return Err(AppError::General(format!(
            "Download failed with status {}: {}",
            response.status(),
            url
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::General(format!("Failed to read .deb response: {}", e)))?;

    let tmp_path = std::env::temp_dir().join("rds-ssm-connect-update.deb");
    std::fs::write(&tmp_path, &bytes)
        .map_err(|e| AppError::General(format!("Failed to write temp .deb: {}", e)))?;

    let _ = app_handle.emit(
        "update-progress",
        serde_json::json!({ "phase": "installing" }),
    );

    let status = std::process::Command::new("pkexec")
        .args(["dpkg", "-i", &tmp_path.to_string_lossy()])
        .status();

    let _ = std::fs::remove_file(&tmp_path);

    match status {
        Ok(s) if s.success() => {
            app_handle.restart();
            #[allow(unreachable_code)]
            Ok(())
        }
        Ok(s) => Err(AppError::General(format!(
            "dpkg install failed (exit {}). Try: sudo dpkg -i <downloaded .deb>",
            s.code().unwrap_or(-1)
        ))),
        Err(e) => Err(AppError::General(format!(
            "Failed to run pkexec: {}. Install cancelled or pkexec not available.",
            e
        ))),
    }
}

/// Upgrade via Homebrew.
#[cfg(target_os = "linux")]
async fn install_brew_update(app_handle: &AppHandle) -> Result<(), AppError> {
    // Find brew binary
    let brew_path = which_brew().ok_or_else(|| {
        AppError::General(
            "Could not find brew. Run manually: brew upgrade rds-ssm-connect".to_string(),
        )
    })?;

    let _ = app_handle.emit(
        "update-progress",
        serde_json::json!({ "phase": "updating" }),
    );

    // Refresh tap first so brew knows about the new version
    let _ = tokio::process::Command::new(&brew_path)
        .args(["update"])
        .output()
        .await;

    let output = tokio::process::Command::new(&brew_path)
        .args(["upgrade", "rds-ssm-connect"])
        .output()
        .await
        .map_err(|e| AppError::General(format!("Failed to run brew: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // brew upgrade exits 0 even when "already installed" — detect no-op
        let combined = format!("{} {}", stdout, stderr).to_lowercase();
        if combined.contains("already installed")
            || combined.contains("up-to-date")
            || combined.contains("up to date")
        {
            return Err(AppError::General(
                "Already up to date in Homebrew. The latest formula may not be published yet. \
                 Try later or run: brew update && brew upgrade rds-ssm-connect"
                    .to_string(),
            ));
        }

        // After brew upgrade, the old Cellar directory is deleted so
        // app_handle.restart() (which re-execs current_exe) would fail.
        // Instead, launch from the Homebrew-linked path.
        let brew_prefix = std::path::Path::new(&brew_path)
            .parent()
            .and_then(|bin| bin.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/home/linuxbrew/.linuxbrew"));
        let new_binary = brew_prefix.join("bin/rds-ssm-connect");

        if let Err(e) = std::process::Command::new(&new_binary).spawn() {
            log::error!("failed to launch updated binary at {}: {}", new_binary.display(), e);
        }
        app_handle.exit(0);
        #[allow(unreachable_code)]
        Ok(())
    } else {
        Err(AppError::General(format!(
            "brew upgrade failed: {}. Try running manually: brew upgrade rds-ssm-connect",
            stderr.trim()
        )))
    }
}

#[cfg(target_os = "linux")]
fn which_brew() -> Option<String> {
    // Check PATH first
    if let Ok(output) = std::process::Command::new("which")
        .arg("brew")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    // Check common Linuxbrew paths
    for path in [
        "/home/linuxbrew/.linuxbrew/bin/brew",
        "/opt/homebrew/bin/brew",
    ] {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

#[tauri::command]
pub async fn install_update(app_handle: AppHandle) -> Result<(), AppError> {
    #[cfg(target_os = "linux")]
    {
        let method = detect_linux_install_method();
        match method {
            LinuxInstallMethod::Deb => {
                // Get the latest version from the updater
                use tauri_plugin_updater::UpdaterExt;
                let updater = app_handle
                    .updater()
                    .map_err(|e| AppError::General(format!("Failed to get updater: {}", e)))?;
                let update = updater
                    .check()
                    .await
                    .map_err(|e| AppError::General(format!("Failed to check for updates: {}", e)))?
                    .ok_or_else(|| AppError::General("No update available".to_string()))?;
                return install_deb_update(&update.version, &app_handle).await;
            }
            LinuxInstallMethod::Homebrew => {
                return install_brew_update(&app_handle).await;
            }
            // AppImage + Unknown: use Tauri's built-in updater
            _ => {}
        }
    }

    // Default path: Tauri updater (works for AppImage on Linux, macOS, Windows)
    use tauri_plugin_updater::UpdaterExt;

    let updater = app_handle
        .updater()
        .map_err(|e| AppError::General(format!("Failed to get updater: {}", e)))?;

    let update = updater
        .check()
        .await
        .map_err(|e| AppError::General(format!("Failed to check for updates: {}", e)))?
        .ok_or_else(|| AppError::General("No update available".to_string()))?;

    // Download with progress events
    let app = app_handle.clone();
    let mut downloaded: u64 = 0;
    let bytes = update
        .download(
            move |chunk_length, content_length| {
                downloaded += chunk_length as u64;
                let _ = app.emit(
                    "update-progress",
                    serde_json::json!({
                        "phase": "downloading",
                        "downloaded": downloaded,
                        "total": content_length,
                    }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| AppError::General(format!("Failed to download update: {}", e)))?;

    let _ = app_handle.emit(
        "update-progress",
        serde_json::json!({ "phase": "installing" }),
    );

    match update.install(&bytes) {
        Ok(()) => {
            app_handle.restart();
        }
        Err(e) => {
            return Err(AppError::General(format!(
                "Install failed: {}. Try downloading manually from the releases page.",
                e
            )));
        }
    }

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
