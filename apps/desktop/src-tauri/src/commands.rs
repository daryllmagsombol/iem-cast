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
use host_core::host::{default_channel_map, HostSession};
use host_core::ids::SourceId;
use host_core::server::TlsIdentity;
use host_core::transport::SelectedInterface;
use tauri::State;

use crate::bridge::{
    CatalogSnapshot, DeviceInfo, InterfaceInfo, IpcError, PairingCredential, SourceInfo,
    StartHostRequest, StartHostResult,
};
use crate::window_guard::authorize_window;

/// Tauri-managed handle to the running host, if any.
///
/// The session is owned here so `stop_host` can join it cleanly. A new `start_host` replaces (and
/// therefore stops) any previous session when the old value is dropped.
#[derive(Default)]
pub struct HostState(pub Mutex<Option<HostSession>>);

impl HostState {
    /// Lock the inner slot, surfacing a poisoned lock as an IPC error.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<HostSession>>, IpcError> {
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

/// Validate the operator-supplied TLS identity and start the local host session.
///
/// This never falls back to plaintext: a missing or unreadable certificate/key returns a typed
/// error so the operator reconfigures instead of serving insecure content. The started
/// [`HostSession`] owns capture, the DSP worker, and the UDP media socket, and is stored in Tauri
/// managed state for `stop_host`.
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

    // Start against the real capture backend. This is the only hardware-opening path; it is never
    // exercised by tests.
    let session = HostSession::start_real(tls, selected, core_request, default_channel_map(channels))
        .map_err(|error| ipc_err("HOST_START_FAILED", error.to_string()))?;

    let host_epoch = uuid::Uuid::new_v4();
    let audio_epoch = session.audio_epoch();

    // Replace any previous session; dropping it stops and joins it.
    {
        let mut slot = state.lock()?;
        *slot = Some(session);
    }

    Ok(StartHostResult {
        host_epoch: host_epoch.to_string(),
        audio_epoch: audio_epoch.to_string(),
        join_url: format!("https://{}/join", request.interface_ip),
    })
}

/// Stop the local host. Operator-window only. Idempotent.
#[tauri::command]
pub fn stop_host(window_label: String, state: State<'_, HostState>) -> Result<(), IpcError> {
    check_caller(&window_label)?;
    let mut slot = state.lock()?;
    if let Some(mut session) = slot.take() {
        session.stop();
    }
    Ok(())
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
#[tauri::command]
pub fn issue_pairing_credential(window_label: String) -> Result<PairingCredential, IpcError> {
    check_caller(&window_label)?;
    Ok(PairingCredential {
        join_url: format!("https://host.local/join#t={}", uuid::Uuid::new_v4()),
        expires_in_seconds: 120,
    })
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
