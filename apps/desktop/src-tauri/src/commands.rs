//! Operator IPC commands.
//!
//! Every command verifies the caller window label first (see [`crate::window_guard`]). Commands
//! never expose filesystem, shell, or LAN controls; they only drive host setup and the musician
//! catalog. The operator source catalog is read-only and derived from the selected device's real
//! channel count, using the same mapping the running host publishes to phones.

use std::sync::Mutex;

use host_core::contract::{
    CaptureRequest as CoreCaptureRequest, InterfaceInfo as CoreInterfaceInfo,
    StartHostRequest as CoreStartHostRequest,
};
use host_core::host::{catalog_from_channel_map, default_channel_map, HostServerHandle, HostSession, ServerConfig};
use host_core::ids::CatalogRevision;
use host_core::server::{OriginPolicy, TlsIdentity};
use host_core::transport::SelectedInterface;
use tauri::State;

use crate::asset_embed::EmbeddedMusicianAssets;
use crate::bridge::{
    CatalogSnapshot, DeviceInfo, HostDefaults, InterfaceInfo, IpcError, OutputDeviceInfo,
    PairingCredential, SourceInfo, StartHostRequest, StartHostResult,
};
use crate::window_guard::authorize_window;

/// The default HTTPS/WSS control port when the operator does not override it.
pub const DEFAULT_CONTROL_PORT: u16 = 8443;

/// Preferred interface name when present.
const PREFERRED_INTERFACE: &str = "en0";

/// How many ancestor directories `host_defaults` walks up looking for `local-certs`.
const CERT_SEARCH_ANCESTORS: usize = 6;

/// The directory pair naming convention for operator TLS material.
const LOCAL_CERTS_DIR: &str = "local-certs";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// A fully started host: the media session and its HTTPS/WSS control server.
///
/// Both halves are owned together so `stop_host` tears down the server first, then the capture
/// session. Dropping this (e.g. when `start_host` replaces it) also stops both, in that order.
pub struct RunningHost {
    server: HostServerHandle,
    session: HostSession,
    /// The active local monitor, kept alive while it plays on the host's own output.
    monitor: Option<std::sync::Arc<host_core::monitor::LocalMonitor>>,
}

impl RunningHost {
    /// Stop the control server first, then the capture/media session. Idempotent.
    fn stop(&mut self) {
        // Detach any monitor tap before the worker stops, so it cannot outlive the session.
        self.session.runtime_handle().clear_monitor();
        self.monitor = None;
        self.server.stop();
        self.session.stop();
    }
}

/// Tauri-managed handle to the running host, if any.
///
/// The session is owned here so `stop_host` can join it cleanly. A new `start_host` replaces (and
/// therefore stops) any previous host when the old value is dropped.
#[derive(Default)]
pub struct HostState(pub Mutex<Option<RunningHost>>);

impl HostState {
    /// Lock the inner slot, surfacing a poisoned lock as an IPC error.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<RunningHost>>, IpcError> {
        self.0
            .lock()
            .map_err(|_| ipc_err("HOST_STATE_POISONED", "host state is unavailable"))
    }
}

fn ipc_err(code: &str, message: impl Into<String>) -> IpcError {
    IpcError {
        code: code.to_string(),
        message: message.into(),
    }
}

fn check_caller(window_label: &str) -> Result<(), IpcError> {
    authorize_window(window_label)
        .map_err(|_| ipc_err("WINDOW_FORBIDDEN", "caller is not the operator window"))
}

/// The exact `https` origin a browser presents for `ip:port`.
///
/// The default HTTPS port is omitted, matching how a browser serializes `Origin` for `:443`.
fn control_origin(ip: std::net::IpAddr, port: u16) -> String {
    if port == 443 {
        format!("https://{ip}")
    } else {
        format!("https://{ip}:{port}")
    }
}

/// Enumerate capture input devices. Operator-window only.
#[tauri::command]
pub fn list_devices(window_label: String) -> Result<Vec<DeviceInfo>, IpcError> {
    check_caller(&window_label)?;
    Ok(host_core::capture::enumerate_input_devices()
        .into_iter()
        .map(|d| DeviceInfo {
            device_id: d.device_id,
            name: d.name,
            is_default: d.is_default,
            input_channels: d.input_channels,
            sample_format: format!("{:?}", d.sample_formats.first()),
            sample_rate_hz: d.sample_rate_hz,
        })
        .collect())
}

/// Enumerate non-loopback IPv4 network interfaces. Operator-window only.
#[tauri::command]
pub fn list_interfaces(window_label: String) -> Result<Vec<InterfaceInfo>, IpcError> {
    check_caller(&window_label)?;
    let interfaces =
        if_addrs::get_if_addrs().map_err(|e| ipc_err("INTERFACES_FAILED", e.to_string()))?;
    let mut out = Vec::new();
    for iface in interfaces {
        if let std::net::IpAddr::V4(v4) = iface.addr.ip() {
            if v4.is_loopback() {
                continue;
            }
            out.push(InterfaceInfo {
                name: iface.name,
                ip_address: v4.to_string(),
                prefix: 0,
            });
        }
    }
    Ok(out)
}

/// Choose the default interface: `en0` when present, otherwise the first candidate offered.
///
/// Input is ordered as the OS enumerated it, so "first non-loopback IPv4" is the caller's ordering
/// contract. The selection is pure so it can be exercised without touching real hardware.
fn select_default_interface(
    candidates: Vec<(String, String)>,
) -> Option<(String, String)> {
    if let Some(preferred) = candidates.iter().find(|(name, _)| name == PREFERRED_INTERFACE) {
        return Some(preferred.clone());
    }
    candidates.into_iter().next()
}

/// Whether a path exists and can be opened for reading; never modifies it.
fn is_readable_file(path: &std::path::Path) -> bool {
    std::fs::File::open(path).is_ok()
}

/// Search `start` and up to `max_ancestors` ancestor directories for a `local-certs` directory
/// containing BOTH `cert.pem` and `key.pem`.
///
/// Returns absolute `(cert, key)` paths only when both files exist and are readable; otherwise
/// `None`. This never creates files or invents a path that does not exist.
fn discover_local_certs(
    start: &std::path::Path,
    max_ancestors: usize,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    for ancestor in start.ancestors().take(max_ancestors) {
        let cert = ancestor.join(LOCAL_CERTS_DIR).join(CERT_FILE);
        let key = ancestor.join(LOCAL_CERTS_DIR).join(KEY_FILE);
        if is_readable_file(&cert) && is_readable_file(&key) {
            return Some((cert, key));
        }
    }
    None
}

/// Discover sensible default host-setup values for the operator.
///
/// This is honest discovery only: the interface is the real OS interface (preferring `en0`) and the
/// certificate/key paths are returned only when both files already exist and are readable. Every
/// field is independently optional, and the frontend keeps the corresponding input editable.
#[tauri::command]
pub fn host_defaults(window_label: String) -> Result<HostDefaults, IpcError> {
    check_caller(&window_label)?;

    // Collect non-loopback IPv4 interfaces in OS order; loopback and IPv6 are skipped.
    let mut candidates: Vec<(String, String)> = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if let std::net::IpAddr::V4(v4) = iface.addr.ip() {
                if v4.is_loopback() {
                    continue;
                }
                candidates.push((iface.name, v4.to_string()));
            }
        }
    }
    let (interface_name, interface_ip) = match select_default_interface(candidates) {
        Some((name, ip)) => (Some(name), Some(ip)),
        None => (None, None),
    };

    // `tauri dev` runs with the crate directory as CWD, so the project root is an ancestor of it.
    let (certificate_path, key_path) = match std::env::current_dir() {
        Ok(cwd) => match discover_local_certs(&cwd, CERT_SEARCH_ANCESTORS) {
            Some((cert, key)) => (
                Some(cert.to_string_lossy().into_owned()),
                Some(key.to_string_lossy().into_owned()),
            ),
            None => (None, None),
        },
        Err(_) => (None, None),
    };

    Ok(HostDefaults {
        interface_name,
        interface_ip,
        certificate_path,
        key_path,
    })
}

/// Validate the operator-supplied TLS identity and start the local host session **and** its
/// HTTPS/WSS control server.
///
/// This never falls back to plaintext: a missing or unreadable certificate/key returns a typed
/// error so the operator reconfigures instead of serving insecure content. The started
/// [`HostSession`] owns capture, the DSP worker, and the UDP media socket; the composed
/// [`HostServerHandle`] serves the musician bundle over trusted TLS. Both are stored in Tauri
/// managed state for `stop_host`. The returned `join_url` is the real URL minted by the server's
/// control actor, never fabricated.
#[tauri::command]
pub fn start_host(
    window_label: String,
    request: StartHostRequest,
    state: State<'_, HostState>,
) -> Result<StartHostResult, IpcError> {
    check_caller(&window_label)?;
    let tls = TlsIdentity::new(
        request.certificate_path.clone(),
        request.key_path.clone(),
    );
    tls.validate_paths().map_err(|_| {
        ipc_err(
            "TLS_IDENTITY_INVALID",
            "certificate or key is missing or unreadable",
        )
    })?;

    let interface_ip: std::net::IpAddr = request
        .interface_ip
        .parse()
        .map_err(|_| ipc_err("INTERFACE_INVALID", "interface address is not a valid IP"))?;

    let port = request.port.unwrap_or(DEFAULT_CONTROL_PORT);
    if port == 0 {
        // An ephemeral port cannot be predicted in the exact-match origin allowlist; refuse rather
        // than bind an origin the browser will never match.
        return Err(ipc_err(
            "PORT_INVALID",
            "a control port must be specified (the default is 8443)",
        ));
    }

    // Resolve the physical channel count from the actual device capabilities (enumeration only,
    // never an audio stream).
    let channels = host_core::capture::enumerate_input_devices()
        .into_iter()
        .find(|d| d.device_id == request.device_id)
        .map(|d| d.input_channels.max(1))
        .unwrap_or(2);

    let core_request = CoreStartHostRequest {
        capture: CoreCaptureRequest {
            device_id: request.device_id.clone(),
            sample_rate_hz: 48_000,
            buffer_frames: 128,
        },
        interface: CoreInterfaceInfo {
            name: request.interface_ip.clone(),
            ip_address: interface_ip,
            prefix: 0,
        },
        certificate_path: request.certificate_path.clone(),
        key_path: request.key_path.clone(),
    };

    let selected = SelectedInterface {
        name: request.interface_ip.clone(),
        ip: interface_ip,
        prefix: 0,
        // 0 tells the host to bind an ephemeral media port and advertise the real bound port; the
        // WebRTC candidate must never be advertised at port 0 or ICE cannot connect.
        port: 0,
    };

    let channel_map = default_channel_map(channels);

    // Start against the real capture backend. This is the only hardware-opening path; it is never
    // exercised by tests.
    let session = HostSession::start_real(
        tls.clone(),
        selected,
        core_request,
        channel_map.clone(),
    )
    .map_err(|error| ipc_err("HOST_START_FAILED", error.to_string()))?;

    let audio_epoch = session.audio_epoch();

    // Compose the HTTPS/WSS control server over the same media session. The origin allowlist holds
    // exactly the https origin a browser would present for this bind address: the default HTTPS
    // port is omitted from the origin, any other port is included.
    let bind = std::net::SocketAddr::new(interface_ip, port);
    let origin = OriginPolicy::new([control_origin(interface_ip, port)]);
    let catalog = catalog_from_channel_map(&channel_map, CatalogRevision(0));
    let config = ServerConfig {
        tls,
        bind,
        origin,
        assets: EmbeddedMusicianAssets::new().into_provider(),
        catalog,
    };
    let server = session
        .start_server(config)
        .map_err(|error| ipc_err("HOST_SERVER_FAILED", error.to_string()))?;

    let join_url = server.join_url().to_string();
    if join_url.is_empty() {
        return Err(ipc_err(
            "HOST_SERVER_FAILED",
            "the control server did not mint a join URL",
        ));
    }

    // Replace any previous host; dropping it stops the server and session in order.
    {
        let mut slot = state.lock()?;
        *slot = Some(RunningHost {
            server,
            session,
            monitor: None,
        });
    }

    Ok(StartHostResult {
        // The running server owns its own host epoch; this correlation id is minted desktop-side
        // because `HostServerHandle` does not yet expose the actor's epoch (see lane report).
        host_epoch: uuid::Uuid::new_v4().to_string(),
        audio_epoch: audio_epoch.to_string(),
        join_url,
    })
}

/// Stop the local host control server and session. Operator-window only. Idempotent.
#[tauri::command]
pub fn stop_host(window_label: String, state: State<'_, HostState>) -> Result<(), IpcError> {
    stop_host_impl(&window_label, &state)
}

/// Testable core of [`stop_host`]: guard the caller, then tear down any running host.
fn stop_host_impl(window_label: &str, state: &HostState) -> Result<(), IpcError> {
    check_caller(window_label)?;
    let mut slot = state.lock()?;
    if let Some(mut host) = slot.take() {
        host.stop();
    }
    Ok(())
}

/// Enumerate local monitor output devices. Operator-window only.
#[tauri::command]
pub fn list_output_devices(window_label: String) -> Result<Vec<OutputDeviceInfo>, IpcError> {
    check_caller(&window_label)?;
    Ok(host_core::monitor::enumerate_output_devices()
        .into_iter()
        .map(|(id, name, is_default)| OutputDeviceInfo {
            id,
            name,
            is_default,
        })
        .collect())
}

/// Resolve the physical input channel count for a device id, the same way `start_host` does.
///
/// Unknown or absent devices resolve to `None`; callers must render an explicit empty state rather
/// than inventing channels.
fn resolve_channels(device_id: Option<&str>) -> Option<u16> {
    let wanted = device_id?;
    host_core::capture::enumerate_input_devices()
        .into_iter()
        .find(|d| d.device_id == wanted)
        .map(|d| d.input_channels.max(1))
}

/// Build an honest, read-only catalog for a resolved channel count.
///
/// The rows come from the same `default_channel_map` + `catalog_from_channel_map` the running host
/// uses, so two channels stay two rows and twenty-two stay twenty-two. This never falls back to a
/// fabricated 22-channel list.
fn catalog_for_channels(channels: u16) -> CatalogSnapshot {
    let catalog = catalog_from_channel_map(&default_channel_map(channels), CatalogRevision(0));
    CatalogSnapshot {
        catalog_revision: catalog.catalog_revision.to_string(),
        sources: catalog
            .sources
            .into_iter()
            .map(|source| SourceInfo {
                source_id: source.source_id.to_string(),
                physical_index: source.physical_index,
                label: source.label,
                // Read-only build: physical presence and cast flag both mirror the device catalog.
                available: source.available,
                available_to_musicians: source.authorized,
            })
            .collect(),
    }
}

/// Build the read-only catalog for a device id, or an explicit empty catalog when it is unknown.
fn catalog_for_device(device_id: Option<&str>) -> CatalogSnapshot {
    match resolve_channels(device_id) {
        Some(channels) => catalog_for_channels(channels),
        // Unknown/absent device: explicitly empty, never a fabricated row set.
        None => CatalogSnapshot {
            catalog_revision: "0".to_string(),
            sources: Vec::new(),
        },
    }
}

/// Read-only operator catalog of the SELECTED device's real input channels. Operator-window only.
///
/// `device_id` identifies the selected capture device. When it is absent or unknown the catalog is
/// explicitly empty; it is never the old hardcoded 22-channel draft.
#[tauri::command]
pub fn source_catalog(
    window_label: String,
    device_id: Option<String>,
) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    Ok(catalog_for_device(device_id.as_deref()))
}

/// Editing the source set is not supported in this build. Operator-window only.
///
/// Returns an explicit typed error instead of a fabricated catalog so the UI can never mistake a
/// no-op for a persisted change.
#[tauri::command]
pub fn set_available_sources(
    window_label: String,
    _ids: Vec<String>,
) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    Err(ipc_err(
        "SOURCE_EDIT_UNSUPPORTED",
        "publishing sources is not supported in this build",
    ))
}

/// Renaming a source is not supported in this build. Operator-window only.
///
/// Returns an explicit typed error instead of a fabricated catalog so the UI can never mistake a
/// no-op for a persisted rename.
#[tauri::command]
pub fn set_source_label(
    window_label: String,
    _id: String,
    _label: String,
) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    Err(ipc_err(
        "SOURCE_EDIT_UNSUPPORTED",
        "saving a source label is not supported in this build",
    ))
}

/// Issue a fresh single-use pairing credential for one phone. Operator-window only.
///
/// The credential is minted by the running host's own authoritative `PairStore`, so it is a real
/// single-use token bound to the live server. The token is returned to the operator UI only and is
/// never logged.
#[tauri::command]
pub fn issue_pairing_credential(
    window_label: String,
    state: State<'_, HostState>,
) -> Result<PairingCredential, IpcError> {
    issue_pairing_credential_impl(&window_label, &state)
}

/// Testable core of [`issue_pairing_credential`].
fn issue_pairing_credential_impl(
    window_label: &str,
    state: &HostState,
) -> Result<PairingCredential, IpcError> {
    check_caller(window_label)?;
    let guard = state.lock()?;
    let host = guard
        .as_ref()
        .ok_or_else(|| ipc_err("HOST_NOT_RUNNING", "no host is running"))?;
    let credential = host.server.issue_pairing_credential().ok_or_else(|| {
        ipc_err(
            "PAIRING_UNAVAILABLE",
            "the running host could not mint a pairing credential",
        )
    })?;
    Ok(PairingCredential {
        join_url: credential.join_url,
        // The IPC DTO uses u32 seconds; the pairing TTL is 120 s, far below the u32 range.
        expires_in_seconds: credential.expires_in_seconds.min(u32::MAX as u64) as u32,
    })
}

/// Start the local monitor on the host's own output device. Operator-window only.
///
/// This is the operator's private listening path, not a phone client and not a mixer return. It
/// taps one listener slot's mix. Physical feedback is possible if the host output is audible to
/// live microphones, so the operator should use headphones when mics are open.
#[tauri::command]
pub fn start_monitor(
    window_label: String,
    slot: u32,
    device_id: Option<String>,
    state: State<'_, HostState>,
) -> Result<(), IpcError> {
    start_monitor_impl(&window_label, slot, device_id.as_deref(), &state)
}

fn start_monitor_impl(
    window_label: &str,
    slot: u32,
    device_id: Option<&str>,
    state: &HostState,
) -> Result<(), IpcError> {
    check_caller(window_label)?;
    let slot = slot as usize;
    if slot >= host_core::audio::engine::MAX_SESSIONS {
        return Err(ipc_err("MONITOR_SLOT_INVALID", "slot is out of range"));
    }
    let mut guard = state.lock()?;
    let host = guard
        .as_mut()
        .ok_or_else(|| ipc_err("HOST_NOT_RUNNING", "no host is running"))?;
    let monitor = std::sync::Arc::new(
        host_core::monitor::LocalMonitor::start(device_id)
            .map_err(|e| ipc_err("MONITOR_START_FAILED", e.to_string()))?,
    );
    // Attach the tap first; only then keep the monitor alive.
    host.session
        .runtime_handle()
        .set_monitor(slot, std::sync::Arc::clone(&monitor) as std::sync::Arc<dyn host_core::monitor::MonitorPort>);
    host.monitor = Some(monitor);
    Ok(())
}

/// Stop the local monitor. Operator-window only. Idempotent.
#[tauri::command]
pub fn stop_monitor(window_label: String, state: State<'_, HostState>) -> Result<(), IpcError> {
    stop_monitor_impl(&window_label, &state)
}

fn stop_monitor_impl(window_label: &str, state: &HostState) -> Result<(), IpcError> {
    check_caller(window_label)?;
    let mut guard = state.lock()?;
    if let Some(host) = guard.as_mut() {
        host.session.runtime_handle().clear_monitor();
        host.monitor = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// Create a unique temporary directory for a test; never leaves shared state behind.
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iem-cast-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn discover_local_certs_walks_up_to_the_project_root() {
        let root = unique_temp_dir("certs-found");
        let project = root.join("proj");
        let certs = project.join("local-certs");
        fs::create_dir_all(&certs).expect("create cert dir");
        fs::write(certs.join("cert.pem"), b"cert").expect("write cert");
        fs::write(certs.join("key.pem"), b"key").expect("write key");
        // Simulate `tauri dev`: the search starts in the crate dir, an ancestor of the root.
        let start = project.join("apps/desktop/src-tauri");
        fs::create_dir_all(&start).expect("create start dir");

        let found = discover_local_certs(&start, 6).expect("must find the project certs");
        assert_eq!(found.0, certs.join("cert.pem"));
        assert_eq!(found.1, certs.join("key.pem"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discover_local_certs_returns_none_when_a_file_is_missing() {
        let root = unique_temp_dir("certs-missing");
        let certs = root.join("local-certs");
        fs::create_dir_all(&certs).expect("create cert dir");
        // Only the certificate exists; discovery must never invent the key.
        fs::write(certs.join("cert.pem"), b"cert").expect("write cert only");

        assert!(discover_local_certs(&root, 6).is_none());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn select_default_interface_prefers_en0_over_the_first_candidate() {
        let candidates = vec![
            ("utun3".to_string(), "10.0.0.2".to_string()),
            ("en0".to_string(), "192.168.1.10".to_string()),
            ("en1".to_string(), "192.168.1.11".to_string()),
        ];
        assert_eq!(
            select_default_interface(candidates),
            Some(("en0".to_string(), "192.168.1.10".to_string()))
        );
    }

    #[test]
    fn select_default_interface_falls_back_to_the_first_when_no_en0() {
        let candidates = vec![
            ("utun3".to_string(), "10.0.0.2".to_string()),
            ("en1".to_string(), "192.168.1.11".to_string()),
        ];
        assert_eq!(
            select_default_interface(candidates),
            Some(("utun3".to_string(), "10.0.0.2".to_string()))
        );
        assert_eq!(select_default_interface(Vec::new()), None);
    }

    #[test]
    fn host_defaults_rejects_a_non_operator_window() {
        let error = host_defaults("musician".to_string()).expect_err("must reject");
        assert_eq!(error.code, "WINDOW_FORBIDDEN");
    }

    #[test]
    fn host_defaults_accepts_the_operator_window() {
        // Discovery may legitimately yield `None` fields in a headless runner; the guard is what
        // matters, and the command must not fail on a machine with no `en0` or no local certs.
        assert!(host_defaults("operator".to_string()).is_ok());
    }

    #[test]
    fn stop_host_is_idempotent_when_no_host_is_running() {
        let state = HostState::default();
        assert!(stop_host_impl("operator", &state).is_ok());
        // A second stop with nothing running is still a successful no-op.
        assert!(stop_host_impl("operator", &state).is_ok());
    }

    #[test]
    fn stop_host_rejects_a_non_operator_window() {
        let state = HostState::default();
        let error = stop_host_impl("musician", &state).expect_err("must reject");
        assert_eq!(error.code, "WINDOW_FORBIDDEN");
    }

    #[test]
    fn issue_pairing_credential_rejects_foreign_callers_and_requires_a_running_host() {
        let state = HostState::default();

        // Non-operator callers are rejected before any host work.
        let denied = issue_pairing_credential_impl("other", &state).expect_err("must reject");
        assert_eq!(denied.code, "WINDOW_FORBIDDEN");

        // With no running host there is nothing to mint from: a typed error, never a token.
        let not_running =
            issue_pairing_credential_impl("operator", &state).expect_err("no host running");
        assert_eq!(not_running.code, "HOST_NOT_RUNNING");
    }

    #[test]
    fn monitor_commands_require_a_running_host() {
        let state = HostState::default();
        // Both monitor controls must reject before touching any device when no host is running.
        let start = start_monitor_impl("operator", 0, None, &state).expect_err("no host");
        assert_eq!(start.code, "HOST_NOT_RUNNING");
        // Stopping with nothing running is a safe no-op.
        assert!(stop_monitor_impl("operator", &state).is_ok());
    }

    #[test]
    fn monitor_commands_reject_foreign_callers() {
        let state = HostState::default();
        assert_eq!(
            start_monitor_impl("musician", 0, None, &state)
                .expect_err("must reject")
                .code,
            "WINDOW_FORBIDDEN"
        );
        assert_eq!(
            stop_monitor_impl("other", &state)
                .expect_err("must reject")
                .code,
            "WINDOW_FORBIDDEN"
        );
    }

    #[test]
    fn start_monitor_rejects_an_out_of_range_slot() {
        let state = HostState::default();
        // Guard the caller + slot range before requiring a host, so a bad slot is its own error.
        let error = start_monitor_impl(
            "operator",
            host_core::audio::engine::MAX_SESSIONS as u32,
            None,
            &state,
        )
        .expect_err("slot out of range");
        assert_eq!(error.code, "MONITOR_SLOT_INVALID");
    }

    #[test]
    fn list_output_devices_rejects_a_non_operator_window() {
        // The command is callable directly (it takes only a label). A non-operator label is denied
        // before any device enumeration happens.
        let error = list_output_devices("musician".to_string()).expect_err("must reject");
        assert_eq!(error.code, "WINDOW_FORBIDDEN");
    }

    #[test]
    fn list_output_devices_accepts_the_operator_window() {
        // Enumeration may legitimately be empty in a headless CI runner; the guard is what matters.
        assert!(list_output_devices("operator".to_string()).is_ok());
    }

    #[test]
    fn output_device_dto_serializes_camel_case() {
        let dto = OutputDeviceInfo {
            id: "dev-1".to_string(),
            name: "Monitor Out".to_string(),
            is_default: true,
        };
        let json = serde_json::to_string(&dto).expect("serializes");
        assert!(json.contains("\"isDefault\""));
        assert!(!json.contains("is_default"));
    }

    #[test]
    fn control_origin_matches_the_browser_serialization() {
        let ip: std::net::IpAddr = "192.0.2.10".parse().unwrap();
        assert_eq!(control_origin(ip, 8443), "https://192.0.2.10:8443");
        // The default HTTPS port must not appear, matching browser `Origin` semantics.
        assert_eq!(control_origin(ip, 443), "https://192.0.2.10");
    }

    #[test]
    fn start_host_request_accepts_an_omitted_port() {
        // Existing callers that never send a port must still deserialize.
        let request: StartHostRequest = serde_json::from_str(
            r#"{"deviceId":"d","interfaceIp":"192.0.2.10","certificatePath":"c.pem","keyPath":"k.pem"}"#,
        )
        .expect("deserializes without a port");
        assert_eq!(request.port, None);
        assert_eq!(request.port.unwrap_or(DEFAULT_CONTROL_PORT), 8443);
    }

    #[test]
    fn catalog_rows_track_the_resolved_device_channel_count() {
        // The row count must equal the device's real channel count: 2 stays 2, 22 stays 22.
        let two = catalog_for_channels(2);
        assert_eq!(two.sources.len(), 2);
        assert_eq!(catalog_for_channels(22).sources.len(), 22);

        // Labels are stable and deterministic, and indices preserve the device mapping.
        assert_eq!(two.sources[0].label, "Channel 1");
        assert_eq!(two.sources[0].physical_index, 0);
        assert_eq!(two.sources[1].label, "Channel 2");
        assert_eq!(two.sources[1].physical_index, 1);
        assert_eq!(two.catalog_revision, "0");
    }

    #[test]
    fn source_catalog_for_an_absent_device_is_empty_and_never_fabricates_rows() {
        // An unknown device id resolves to no channels: an explicit empty catalog, never 22 rows.
        let snapshot = catalog_for_device(Some("no-such-device"));
        assert_eq!(snapshot.catalog_revision, "0");
        assert!(snapshot.sources.is_empty());
    }

    #[test]
    fn source_catalog_without_a_device_is_empty() {
        let snapshot = catalog_for_device(None);
        assert_eq!(snapshot.catalog_revision, "0");
        assert!(snapshot.sources.is_empty());
    }

    #[test]
    fn source_catalog_rejects_a_non_operator_window() {
        let error = source_catalog("musician".to_string(), None).expect_err("must reject");
        assert_eq!(error.code, "WINDOW_FORBIDDEN");
    }

    #[test]
    fn source_mutations_return_an_explicit_unsupported_error() {
        // The UI keeps these disabled; the commands must fail loudly, never fabricate a catalog.
        assert_eq!(
            set_available_sources("operator".to_string(), vec![])
                .expect_err("must reject")
                .code,
            "SOURCE_EDIT_UNSUPPORTED"
        );
        assert_eq!(
            set_source_label("operator".to_string(), "id".to_string(), "label".to_string())
                .expect_err("must reject")
                .code,
            "SOURCE_EDIT_UNSUPPORTED"
        );
    }

    #[test]
    fn source_mutations_reject_a_non_operator_window() {
        assert_eq!(
            set_available_sources("musician".to_string(), vec![])
                .expect_err("must reject")
                .code,
            "WINDOW_FORBIDDEN"
        );
        assert_eq!(
            set_source_label("musician".to_string(), "id".to_string(), "label".to_string())
                .expect_err("must reject")
                .code,
            "WINDOW_FORBIDDEN"
        );
    }
}
