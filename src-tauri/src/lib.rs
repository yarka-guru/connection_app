pub mod aws;
#[cfg(feature = "gui")]
mod commands;
pub mod config;
pub mod error;
pub mod history;
#[cfg(feature = "gui")]
pub mod sandbox;
#[cfg(feature = "gui")]
pub mod tray;
pub mod tunnel;

#[cfg(feature = "gui")]
pub fn run() {
    use std::sync::Arc;
    use tauri::{Listener, Manager};
    use tokio::sync::Mutex;
    use tunnel::manager::TunnelManager;

    // Initialize logging so log::info!/warn!/error! produce output in dev mode
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Migrate legacy ~/.rds-ssm-connect/ → ~/.connection-app/ on first launch
            config::projects::migrate_legacy_config();

            let tunnel_manager = TunnelManager::new(app.handle().clone());
            app.manage(Arc::new(Mutex::new(tunnel_manager)));

            // Activate AWS directory bookmark if sandboxed and bookmark exists
            let aws_access = if sandbox::is_sandboxed() && sandbox::has_stored_bookmark(app.handle()) {
                match sandbox::activate_aws_dir_access(app.handle()) {
                    Ok(access) => {
                        log::info!("Activated AWS directory access: {}", access.aws_dir_path.display());
                        Some(access)
                    }
                    Err(e) => {
                        log::warn!("Failed to activate AWS directory bookmark: {}", e);
                        None
                    }
                }
            } else {
                None
            };
            app.manage(Arc::new(std::sync::RwLock::new(aws_access)) as commands::system::AwsDirState);

            // Set up system tray
            if let Err(e) = tray::setup_tray(app.handle()) {
                log::error!("Failed to set up system tray: {}", e);
            }

            // Listen for connection state changes to refresh the tray
            let app_handle = app.handle().clone();
            app.listen("disconnected", move |_event| {
                tray::refresh_tray(&app_handle);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide window on close instead of quitting (keep app in tray)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Project config commands
            commands::projects::list_projects,
            commands::projects::list_project_configs,
            commands::projects::save_project_config,
            commands::projects::delete_project_config,
            // Connection commands
            commands::connection::connect,
            commands::connection::disconnect,
            commands::connection::disconnect_all,
            commands::connection::get_active_connections_list,
            commands::connection::get_used_ports,
            commands::connection::sso_login,
            // Profile commands
            commands::profiles::list_profiles,
            commands::profiles::read_aws_config,
            commands::profiles::save_aws_profile,
            commands::profiles::delete_aws_profile,
            commands::profiles::get_raw_aws_config,
            commands::profiles::save_raw_aws_config,
            // Saved connections commands
            commands::saved::load_saved_connections,
            commands::saved::save_connection,
            commands::saved::delete_saved_connection,
            commands::saved::update_saved_connection,
            commands::saved::reorder_saved_connections,
            commands::saved::update_saved_connection_last_used,
            commands::saved::move_connection_to_group,
            commands::saved::rename_connection_group,
            commands::saved::delete_connection_group,
            // Tray commands
            commands::tray::refresh_tray_menu,
            // System commands
            commands::system::check_for_updates,
            commands::system::install_update,
            commands::system::get_current_version,
            commands::system::open_url,
            commands::system::quit_app,
            commands::system::get_sandbox_status,
            commands::system::grant_aws_access,
            commands::system::check_migration_available,
            commands::system::import_projects_file,
            commands::system::export_projects_file,
            commands::system::export_saved_connections,
            commands::system::import_saved_connections,
            commands::system::get_connection_history,
            commands::system::clear_connection_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
