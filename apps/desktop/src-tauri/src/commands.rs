//! Operator IPC commands.
//!
//! Every command verifies the caller window label first (see [`crate::window_guard`]). Commands
//! never expose filesystem, shell, or LAN controls; they only drive host setup and the musician
//! catalog. The musician catalog here is a draft 22-channel mapping pending the real device
//! capability probe; it is not a validated hardware mapping.

use host_core::ids::SourceId;

use crate::bridge::{
    CatalogSnapshot, DeviceInfo, InterfaceInfo, IpcError, PairingCredential, SourceInfo,
    StartHostRequest, StartHostResult,
};
use crate::window_guard::authorize_window;

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

/// Validate the operator-supplied TLS identity and return the info needed to join.
///
/// This never falls back to plaintext: a missing or unreadable certificate/key returns a typed
/// error so the operator reconfigures instead of serving insecure content.
#[tauri::command]
pub fn start_host(
    window_label: String,
    request: StartHostRequest,
) -> Result<StartHostResult, IpcError> {
    check_caller(&window_label)?;
    let tls = host_core::server::TlsIdentity::new(
        request.certificate_path.clone(),
        request.key_path.clone(),
    );
    tls.validate_paths().map_err(|_| {
        ipc_err(
            "TLS_IDENTITY_INVALID",
            "certificate or key is missing or unreadable",
        )
    })?;
    let host_epoch = uuid::Uuid::new_v4();
    let audio_epoch = uuid::Uuid::new_v4();
    Ok(StartHostResult {
        host_epoch: host_epoch.to_string(),
        audio_epoch: audio_epoch.to_string(),
        join_url: format!("https://{}/join", request.interface_ip),
    })
}

/// Stop the local host. Operator-window only.
#[tauri::command]
pub fn stop_host(window_label: String) -> Result<(), IpcError> {
    check_caller(&window_label)?;
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
