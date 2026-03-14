use crate::commands::saved::SavedConnection;
use crate::tunnel::manager::{ActiveConnectionInfo, TunnelManager};
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// Menu item ID prefixes
const ID_SHOW_HIDE: &str = "show-hide";
const ID_DISCONNECT_PREFIX: &str = "disconnect:";
const ID_QUICK_CONNECT_PREFIX: &str = "quick-connect:";
const ID_QUIT: &str = "quit";

/// Create a 22x22 RGBA icon filled with a given color and a simple "DB" shape.
/// For the default state, we use a neutral gray; for connected, green.
fn make_tray_icon_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    let size: usize = 22;
    let mut pixels = vec![0u8; size * size * 4];

    // Draw a simple database/cylinder icon shape
    // Top ellipse: rows 3-6, columns 5-16
    // Body: rows 6-15, columns 5-16
    // Bottom ellipse: rows 15-18, columns 5-16

    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let cx = 11.0f64;

            // Database cylinder shape
            let in_body = (5..=16).contains(&x) && (5..=17).contains(&y);
            let in_top_ellipse = {
                let ex = (x as f64 - cx) / 6.0;
                let ey = (y as f64 - 5.0) / 2.5;
                ex * ex + ey * ey <= 1.0
            };
            let in_bottom_ellipse = {
                let ex = (x as f64 - cx) / 6.0;
                let ey = (y as f64 - 17.0) / 2.5;
                ex * ex + ey * ey <= 1.0
            };
            let in_middle_ellipse = {
                let ex = (x as f64 - cx) / 6.0;
                let ey = (y as f64 - 11.0) / 2.5;
                ex * ex + ey * ey <= 1.0 && (9..=13).contains(&y)
            };
            // Side walls
            let in_side = (x == 5 || x == 16) && (5..=17).contains(&y);

            let alpha = if in_top_ellipse || in_bottom_ellipse || in_body {
                // Determine if it's an outline or fill
                let is_outline = in_top_ellipse && (y <= 3 || y >= 7)
                    || in_bottom_ellipse
                    || in_side
                    || in_middle_ellipse;

                if is_outline || in_top_ellipse {
                    255u8
                } else if in_body && x > 5 && x < 16 {
                    200u8
                } else {
                    0u8
                }
            } else {
                0u8
            };

            if alpha > 0 {
                pixels[idx] = r;
                pixels[idx + 1] = g;
                pixels[idx + 2] = b;
                pixels[idx + 3] = alpha;
            }
        }
    }

    pixels
}

/// Get the default (neutral) tray icon.
fn default_icon() -> Image<'static> {
    let rgba = make_tray_icon_rgba(180, 180, 190);
    Image::new_owned(rgba, 22, 22)
}

/// Get the connected (green) tray icon.
fn connected_icon() -> Image<'static> {
    let rgba = make_tray_icon_rgba(80, 200, 120);
    Image::new_owned(rgba, 22, 22)
}

/// Set up the system tray icon and event handlers.
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_tray_menu(app, &[], &[])?;

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(default_icon())
        .menu(&menu)
        .tooltip("ConnectionApp")
        .on_tray_icon_event(|tray_icon, event| {
            use tauri::tray::TrayIconEvent;
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                ..
            } = event
            {
                toggle_window_visibility(tray_icon.app_handle());
            }
        })
        .on_menu_event(|app_handle, event| {
            let id = event.id().as_ref();
            handle_menu_event(app_handle, id);
        })
        .build(app)?;

    // Store the tray reference so we can update it later
    app.manage(TrayState {
        _tray: tray,
    });

    Ok(())
}

struct TrayState {
    _tray: tauri::tray::TrayIcon,
}

/// Handle menu item clicks.
fn handle_menu_event(app_handle: &AppHandle, id: &str) {
    if id == ID_SHOW_HIDE {
        toggle_window_visibility(app_handle);
    } else if id == ID_QUIT {
        // Disconnect all and quit
        let app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(manager) = app.try_state::<Arc<Mutex<TunnelManager>>>() {
                let mgr = manager.lock().await;
                let _ = mgr.disconnect_all().await;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            app.exit(0);
        });
    } else if let Some(conn_id) = id.strip_prefix(ID_DISCONNECT_PREFIX) {
        let app = app_handle.clone();
        let conn_id = conn_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Some(manager) = app.try_state::<Arc<Mutex<TunnelManager>>>() {
                let mgr = manager.lock().await;
                let _ = mgr.disconnect(&conn_id).await;
            }
            // Tray will be refreshed by the disconnect event handler
        });
    } else if let Some(saved_id) = id.strip_prefix(ID_QUICK_CONNECT_PREFIX) {
        // Emit a custom event so the frontend can handle the quick connect
        let _ = app_handle.emit("tray-quick-connect", saved_id);
        // Also show the window so the user can see the connection progress
        show_window(app_handle);
    }
}

/// Toggle main window visibility.
fn toggle_window_visibility(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            show_window(app_handle);
        }
    }
}

/// Show and focus the main window.
fn show_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Build the tray menu with current state.
fn build_tray_menu(
    app: &AppHandle,
    active_connections: &[ActiveConnectionInfo],
    saved_connections: &[SavedConnection],
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let menu = MenuBuilder::new(app);

    // App name and version (disabled label)
    let header = MenuItemBuilder::with_id("header", format!("ConnectionApp v{}", CURRENT_VERSION))
        .enabled(false)
        .build(app)?;

    // Show/Hide window
    let is_visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(true))
        .unwrap_or(true);
    let show_hide_label = if is_visible { "Hide Window" } else { "Show Window" };
    let show_hide = MenuItemBuilder::with_id(ID_SHOW_HIDE, show_hide_label).build(app)?;

    let mut menu = menu.item(&header).item(&show_hide);

    // Active connections section
    if !active_connections.is_empty() {
        let sep = PredefinedMenuItem::separator(app)?;
        let active_header =
            MenuItemBuilder::with_id("active-header", "Active Connections")
                .enabled(false)
                .build(app)?;
        menu = menu.item(&sep).item(&active_header);

        for conn in active_connections {
            let label = format!(
                "  {} ({}) :{}",
                conn.project_key, conn.profile, conn.local_port
            );
            let submenu = SubmenuBuilder::with_id(
                app,
                format!("active-sub:{}", conn.id),
                &label,
            )
            .item(
                &MenuItemBuilder::with_id(
                    format!("{}{}", ID_DISCONNECT_PREFIX, conn.id),
                    "Disconnect",
                )
                .build(app)?,
            )
            .build()?;
            menu = menu.item(&submenu);
        }
    }

    // Saved connections section
    if !saved_connections.is_empty() {
        let sep = PredefinedMenuItem::separator(app)?;
        let saved_header =
            MenuItemBuilder::with_id("saved-header", "Quick Connect")
                .enabled(false)
                .build(app)?;
        menu = menu.item(&sep).item(&saved_header);

        for saved in saved_connections {
            let label = format!("  {}", saved.name);
            let item = MenuItemBuilder::with_id(
                format!("{}{}", ID_QUICK_CONNECT_PREFIX, saved.id),
                &label,
            )
            .build(app)?;
            menu = menu.item(&item);
        }
    }

    // Separator + Quit
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "Quit").build(app)?;
    menu = menu.item(&sep).item(&quit);

    menu.build()
}

/// Refresh the tray menu and icon based on current state.
/// Call this whenever connections change.
pub fn refresh_tray(app_handle: &AppHandle) {
    let app = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let active_connections = if let Some(manager) = app.try_state::<Arc<Mutex<TunnelManager>>>()
        {
            let mgr = manager.lock().await;
            mgr.get_active_connections().await
        } else {
            vec![]
        };

        let saved_connections = load_saved_connections_for_tray(&app);

        let has_active = !active_connections.is_empty();

        if let Ok(menu) = build_tray_menu(&app, &active_connections, &saved_connections)
            && let Some(tray) = app.tray_by_id("main-tray") {
                let _ = tray.set_menu(Some(menu));

                // Update icon based on connection state
                if has_active {
                    let _ = tray.set_icon(Some(connected_icon()));
                    let _ = tray.set_tooltip(Some(format!(
                        "ConnectionApp - {} active",
                        active_connections.len()
                    )));
                } else {
                    let _ = tray.set_icon(Some(default_icon()));
                    let _ = tray.set_tooltip(Some("ConnectionApp"));
                }
            }
    });
}

/// Load saved connections from the store (non-async, for tray menu building).
fn load_saved_connections_for_tray(app: &AppHandle) -> Vec<SavedConnection> {
    use tauri_plugin_store::StoreExt;

    let store = match app.store("connections.json") {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    store
        .get("savedConnections")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}
