use crate::error::AppError;
use tauri::AppHandle;

/// Tauri command to refresh the tray menu from the frontend.
/// Call this after connecting, disconnecting, or saving/deleting saved connections.
#[tauri::command]
pub async fn refresh_tray_menu(app_handle: AppHandle) -> Result<(), AppError> {
    crate::tray::refresh_tray(&app_handle);
    Ok(())
}
