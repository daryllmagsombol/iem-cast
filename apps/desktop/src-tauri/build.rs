//! Tauri build script.
//!
//! Registers the operator commands in the application ACL manifest. A capability file alone does
//! **not** restrict custom commands: commands registered through `invoke_handler` are usable by
//! all application windows by default. Declaring them here generates the allow/deny permissions
//! that the operator capability references.

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "list_devices",
            "list_interfaces",
            "host_defaults",
            "start_host",
            "stop_host",
            "source_catalog",
            "set_available_sources",
            "set_source_label",
            "issue_pairing_credential",
            "list_output_devices",
            "start_monitor",
            "stop_monitor",
        ])),
    )
    .expect("failed to run tauri-build");
}
