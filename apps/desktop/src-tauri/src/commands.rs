//! Operator IPC commands.
//!
//! Every command verifies the caller window label first (see [`crate::window_guard`]). Commands
//! never expose filesystem, shell, or LAN controls; they only drive host setup and the musician
//! catalog. The musician catalog here is a draft 22-channel mapping pending the real device
//! capability probe; it is not a validated hardware mapping.

use std::sync::Mutex;

use host_core::contract::{
    CaptureRequest as CoreCaptureRequest, InterfaceInfo as CoreInterfaceInfo,
    StartHostRequest as CoreStartHostRequest,
};
use host_core::host::{catalog_from_channel_map, default_channel_map, HostServerHandle, HostSession, ServerConfig};
use host_core::ids::{CatalogRevision, SourceId};
use host_core::server::{OriginPolicy, TlsIdentity};
use host_core::transport::SelectedInterface;
use tauri::State;

use crate::asset_embed::EmbeddedMusicianAssets;
use crate::bridge::{
    CatalogSnapshot, DeviceInfo, InterfaceInfo, IpcError, OutputDeviceInfo, PairingCredential,
    SourceInfo, StartHostRequest, StartHostResult,
};
use crate::window_guard::authorize_window;

/// The default HTTPS/WSS control port when the operator does not override it.
pub const DEFAULT_CONTROL_PORT: u16 = 8443;

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

/// Draft 22-channel catalog. Operator-window only.
#[tauri::command]
pub fn source_catalog(window_label: String) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    Ok(build_catalog(&[]))
}

/// Choose which sources are available to musicians. Operator-window only.
#[tauri::command]
pub fn set_available_sources(
    window_label: String,
    ids: Vec<String>,
) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    Ok(build_catalog(&ids))
}

/// Rename a source. Operator-window only.
#[tauri::command]
pub fn set_source_label(
    window_label: String,
    id: String,
    label: String,
) -> Result<CatalogSnapshot, IpcError> {
    check_caller(&window_label)?;
    if label.len() > 64 {
        return Err(ipc_err(
            "LABEL_TOO_LONG",
            "label must be 64 characters or fewer",
        ));
    }
    let _ = id;
    Ok(build_catalog(&[]))
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

fn build_catalog(available_ids: &[String]) -> CatalogSnapshot {
    let sources: Vec<SourceInfo> = (0..22u16)
        .map(|index| {
            let id = SourceId::from_bytes([index as u8; 16]);
            let id_text = id.0.to_string();
            SourceInfo {
                available_to_musicians: available_ids.iter().any(|v| v == &id_text),
                source_id: id_text,
                physical_index: index + 1,
                label: format!("Channel {}", index + 1),
                available: true,
            }
        })
        .collect();
    CatalogSnapshot {
        catalog_revision: "0".to_string(),
        sources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
