//! IEM Cast desktop shell.
//!
//! A minimal Tauri operator window that composes the `host-core` process and exposes a narrow,
//! allowlisted IPC bridge. The LAN musician page is served by host-core over trusted HTTPS and
//! never gains privileged access.

pub mod asset_embed;
pub mod bridge;
pub mod commands;
pub mod source_settings;
pub mod window_guard;

pub use asset_embed::EmbeddedMusicianAssets;
pub use window_guard::{authorize_call, authorize_window, WindowDenied, OPERATOR_WINDOW_LABEL};

use std::sync::Arc;

use tauri::Manager;

/// The fixed application-local settings filename (resolved under the app local data dir).
pub const SOURCE_SETTINGS_FILE: &str = "source-settings.json";

/// Resolve the production settings path once, under the app-local data directory.
///
/// This uses Tauri's `Manager` path resolver (never the CWD or a test path) and creates the parent
/// directory if needed. Tests never call this; they inject a temporary path.
pub fn resolve_source_settings_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("could not resolve the app local data dir: {error}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create the app local data dir: {error}"))?;
    Ok(dir.join(SOURCE_SETTINGS_FILE))
}

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::HostState::default())
        .setup(|app| {
            // Resolve the fixed production settings path ONCE and inject the manager. Production
            // uses the real CPAL capture identity; no test path or hardware is involved here.
            let path = resolve_source_settings_path(app.handle())?;
            let settings = source_settings::SourceSettingsState::open(
                path,
                Arc::new(source_settings::SystemCaptureIdentity),
            );
            app.manage(settings);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::list_interfaces,
            commands::host_defaults,
            commands::start_host,
            commands::stop_host,
            commands::source_catalog,
            commands::set_available_sources,
            commands::set_source_label,
            commands::issue_pairing_credential,
            commands::list_output_devices,
            commands::start_monitor,
            commands::stop_monitor,
            commands::read_source_settings,
            commands::save_source_defaults,
            commands::save_source_links,
        ])
        .run(tauri::generate_context!())
        .expect("error while running IEM Cast desktop");
}
