//! IEM Cast desktop shell.
//!
//! A minimal Tauri operator window that composes the `host-core` process and exposes a narrow,
//! allowlisted IPC bridge. The LAN musician page is served by host-core over trusted HTTPS and
//! never gains privileged access.

pub mod asset_embed;
pub mod bridge;
pub mod commands;
pub mod window_guard;

pub use asset_embed::EmbeddedMusicianAssets;
pub use window_guard::{authorize_window, WindowDenied, OPERATOR_WINDOW_LABEL};

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::HostState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::list_interfaces,
            commands::start_host,
            commands::stop_host,
            commands::source_catalog,
            commands::set_available_sources,
            commands::set_source_label,
            commands::issue_pairing_credential,
            commands::list_output_devices,
            commands::start_monitor,
            commands::stop_monitor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running IEM Cast desktop");
}
