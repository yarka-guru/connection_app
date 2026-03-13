pub mod aws;
#[cfg(feature = "gui")]
mod commands;
pub mod config;
pub mod error;
#[cfg(feature = "gui")]
pub mod sandbox;
pub mod tunnel;

#[cfg(feature = "gui")]
pub fn run() {
    use std::sync::Arc;
    use tauri::Manager;
    use tokio::sync::Mutex;
    use tunnel::manager::TunnelManager;

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
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

            Ok(())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
