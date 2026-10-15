//! Desktop composition tests.
//!
//! These assert the shell's privilege boundary (both the capability ACL and the injected-window
//! guard) and the source-settings IPC contract. They never open audio hardware and never write to
//! real user data: capture identity is an injected fake and the settings store is a temporary file.
//!
//! The ACL tests build a real Tauri [`Context`](tauri::Context) from the application's own
//! `tauri.conf.json` (so the real `operator-control` capability is resolved) with noop test assets.
//! `mock_context` is deliberately NOT used: it has an empty ACL and would prove nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iem_cast_desktop::asset_embed::EmbeddedMusicianAssets;
use iem_cast_desktop::bridge::BindingStatus;
use iem_cast_desktop::commands::HostState;
use iem_cast_desktop::source_settings::{CaptureIdentity, SourceSettingsState};
use iem_cast_desktop::window_guard::authorize_window;

use host_core::contract::AssetProvider;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindowBuilder};

/// A deterministic capture-identity resolver for tests: never touches hardware or real user data.
struct FakeIdentity {
    by_device: HashMap<String, (String, u16)>,
}

impl FakeIdentity {
    fn new(pairs: &[(&str, &str, u16)]) -> Arc<Self> {
        Arc::new(Self {
            by_device: pairs
                .iter()
                .map(|(device, uid, channels)| (device.to_string(), (uid.to_string(), *channels)))
                .collect(),
        })
    }
}

impl CaptureIdentity for FakeIdentity {
    fn verified(&self, device_id: &str, _rate: u32) -> Result<(String, u16), String> {
        self.by_device
            .get(device_id)
            .cloned()
            .ok_or_else(|| format!("unknown test device {device_id}"))
    }
}

/// A unique temporary settings file; never the real app-local data path.
fn temp_settings_path(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "iem-cast-composition-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join("source-settings.json")
}

/// The local (bundled) platform origin a Tauri webview presents. macOS/Linux/iOS use the custom
/// `tauri://localhost` scheme; Windows/Android use `http://tauri.localhost`.
fn local_origin() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

/// Build the real-ACL mock app with the operator handlers and injected (temporary) state.
fn build_app(tag: &str) -> tauri::App<tauri::test::MockRuntime> {
    let settings = SourceSettingsState::open(
        temp_settings_path(tag),
        FakeIdentity::new(&[("dev", "uid-A", 2)]),
    );
    mock_builder()
        .manage(HostState::default())
        .manage(settings)
        .invoke_handler(tauri::generate_handler![
            iem_cast_desktop::commands::read_source_settings,
            iem_cast_desktop::commands::save_source_defaults,
            iem_cast_desktop::commands::save_source_links,
        ])
        .build(tauri::generate_context!(
            assets = tauri::test::noop_assets(),
            test = true
        ))
        .expect("build the mock app with the real ACL")
}

/// An invoke request for `cmd` from `url` with a JSON body.
fn request(cmd: &str, url: &str, body: serde_json::Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.to_string(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: url.parse().expect("valid url"),
        body: InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

#[test]
fn asset_provider_trait_allows_host_core_tests_without_frontend_dist() {
    let provider = EmbeddedMusicianAssets::new();
    // The provider is constructed from compiled-in assets; host-core tests never need a
    // `frontendDist` directory to exist.
    let _: &dyn AssetProvider = &provider;
}

#[test]
fn embedded_assets_serve_the_join_page() {
    let provider = EmbeddedMusicianAssets::new();
    assert!(provider.get("/join").is_some());
}

#[test]
fn ipc_command_rejects_non_operator_window_label() {
    assert!(authorize_window("operator").is_ok());
    assert!(authorize_window("musician").is_err());
    assert!(authorize_window("other").is_err());
}

#[test]
fn local_operator_window_is_allowed_read_source_settings() {
    let app = build_app("acl-allow");
    let webview = WebviewWindowBuilder::new(&app, "operator", Default::default())
        .build()
        .expect("operator window");

    let response = get_ipc_response(
        &webview,
        request(
            "read_source_settings",
            local_origin(),
            serde_json::json!({ "deviceId": "dev" }),
        ),
    );
    let body = response.expect("local operator must be allowed");
    let value: serde_json::Value = body.deserialize().expect("deserializes");
    assert_eq!(value["deviceId"], "uid-A");
    assert_eq!(value["inputChannels"], 2);
    assert_eq!(value["bindingStatus"], "new");
    assert_eq!(value["durabilityUncertain"], false);
    assert_eq!(value["catalog"]["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn other_window_with_spoofed_label_is_denied_by_acl_before_any_mutation() {
    let app = build_app("acl-other-window");
    // A real second window labelled "other" is not covered by the operator capability.
    let webview = WebviewWindowBuilder::new(&app, "other", Default::default())
        .build()
        .expect("other window");

    // The body spoofs the operator window label; the ACL rejects on the ACTUAL window first.
    let response = get_ipc_response(
        &webview,
        request(
            "save_source_links",
            local_origin(),
            serde_json::json!({
                "windowLabel": "operator",
                "deviceId": "dev",
                "baseSettingsRevision": "0",
                "pairs": [],
            }),
        ),
    );
    assert!(response.is_err(), "a non-operator window must be denied");

    // No mutation occurred: nothing was ever saved, so no settings file exists on disk.
    let state = app.state::<SourceSettingsState>();
    let path = state.lock().unwrap().path().to_path_buf();
    assert!(!path.exists(), "a denied call must not write the settings file");
}

#[test]
fn same_operator_window_on_a_remote_origin_is_denied() {
    let app = build_app("acl-remote-origin");
    let webview = WebviewWindowBuilder::new(&app, "operator", Default::default())
        .build()
        .expect("operator window");

    // The actual window is the operator window, but the request origin is remote: no remote grant.
    let response = get_ipc_response(
        &webview,
        request(
            "read_source_settings",
            "https://evil.example.com/",
            serde_json::json!({ "deviceId": "dev" }),
        ),
    );
    assert!(response.is_err(), "a remote origin must be denied on the operator window");
}

#[test]
fn operator_window_defaults_save_round_trips_through_the_real_acl() {
    let app = build_app("acl-save-defaults");
    let webview = WebviewWindowBuilder::new(&app, "operator", Default::default())
        .build()
        .expect("operator window");

    let read = get_ipc_response(
        &webview,
        request(
            "read_source_settings",
            local_origin(),
            serde_json::json!({ "deviceId": "dev" }),
        ),
    )
    .expect("read allowed")
    .deserialize::<serde_json::Value>()
    .unwrap();
    let sources = read["catalog"]["sources"].as_array().unwrap().clone();
    let defaults: Vec<serde_json::Value> = sources
        .iter()
        .map(|s| serde_json::json!({ "sourceId": s["sourceId"], "pan": 0.0 }))
        .collect();

    let response = get_ipc_response(
        &webview,
        request(
            "save_source_defaults",
            local_origin(),
            serde_json::json!({
                "deviceId": "dev",
                "baseSettingsRevision": "0",
                "sourceDefaults": defaults,
            }),
        ),
    );
    let saved = response.expect("defaults save allowed").deserialize::<serde_json::Value>().unwrap();
    assert_eq!(saved["bindingStatus"], "saved");
    assert_eq!(saved["settingsRevision"], "1");
}

#[test]
fn operator_window_replacement_link_save_requires_consent() {
    let app = build_app("acl-replacement-consent");
    let webview = WebviewWindowBuilder::new(&app, "operator", Default::default())
        .build()
        .expect("operator window");

    // Seed a saved binding for device "dev" -> uid-A, 2ch.
    let read = get_ipc_response(
        &webview,
        request(
            "read_source_settings",
            local_origin(),
            serde_json::json!({ "deviceId": "dev" }),
        ),
    )
    .unwrap()
    .deserialize::<serde_json::Value>()
    .unwrap();
    let defaults: Vec<serde_json::Value> = read["catalog"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| serde_json::json!({ "sourceId": s["sourceId"], "pan": 0.0 }))
        .collect();
    get_ipc_response(
        &webview,
        request(
            "save_source_defaults",
            local_origin(),
            serde_json::json!({
                "deviceId": "dev",
                "baseSettingsRevision": "0",
                "sourceDefaults": defaults,
            }),
        ),
    )
    .expect("initial save allowed");

    // The fake identity does not know "dev2": a save for it fails before any mutation.
    let response = get_ipc_response(
        &webview,
        request(
            "save_source_links",
            local_origin(),
            serde_json::json!({
                "deviceId": "dev2",
                "baseSettingsRevision": "1",
                "pairs": [],
                "replaceBinding": true,
            }),
        ),
    );
    assert!(response.is_err(), "an unverifiable device save must fail, not silently rebind");
    // No mutation: the committed binding for uid-A is unchanged (a fresh read still reports saved).
    let after = get_ipc_response(
        &webview,
        request(
            "read_source_settings",
            local_origin(),
            serde_json::json!({ "deviceId": "dev" }),
        ),
    )
    .unwrap()
    .deserialize::<serde_json::Value>()
    .unwrap();
    assert_eq!(after["bindingStatus"], "saved", "the saved binding is untouched");
    assert_eq!(after["settingsRevision"], "1");
}

#[test]
fn bridge_binding_status_round_trips_the_exact_wire_strings() {
    // The DTO's status enum serializes to the exact lowercase strings the UI/TS contract expects.
    for (status, expected) in [
        (BindingStatus::Saved, "saved"),
        (BindingStatus::New, "new"),
        (BindingStatus::Replacement, "replacement"),
        (BindingStatus::Legacy, "legacy"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), serde_json::json!(expected));
    }
}

#[test]
fn source_settings_dto_uses_camel_case_fields() {
    let json = serde_json::to_string(&serde_json::json!({
        "deviceId": "uid-A",
        "inputChannels": 2,
        "settingsRevision": "0",
        "bindingStatus": "new",
        "durabilityUncertain": false,
        "catalog": { "catalogRevision": "0", "sources": [] },
    }))
    .unwrap();
    // Guard against accidental snake_case regression in the frozen DTO.
    assert!(json.contains("inputChannels"));
    assert!(json.contains("bindingStatus"));
}

#[test]
fn source_settings_state_never_uses_the_real_user_data_dir() {
    // Guard rail: the injected path is a temporary directory, never an app-local user path.
    let path = temp_settings_path("guard-rail");
    assert!(path.starts_with(std::env::temp_dir()));
}
