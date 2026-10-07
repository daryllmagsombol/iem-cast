//! Operator IPC bridge types.
//!
//! These mirror `apps/web/src/desktop-bridge/contracts.ts`. Only the bundled operator webview
//! may call them; they never expose filesystem, shell, or LAN controls.

use serde::{Deserialize, Serialize};

/// A capture input device offered to the operator.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub is_default: bool,
    pub input_channels: u16,
    pub sample_format: String,
    pub sample_rate_hz: Option<u32>,
}

/// A selectable host network interface.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceInfo {
    pub name: String,
    pub ip_address: String,
    pub prefix: u8,
}

/// Request to start the local host.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostRequest {
    pub device_id: String,
    pub interface_ip: String,
    pub certificate_path: String,
    pub key_path: String,
}

/// Result of starting the local host.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostResult {
    pub host_epoch: String,
    pub audio_epoch: String,
    pub join_url: String,
}

/// A single-use pairing credential for one phone.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCredential {
    pub join_url: String,
    pub expires_in_seconds: u32,
}

/// A source the operator may make available to musicians.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub source_id: String,
    pub physical_index: u16,
    pub label: String,
    pub available: bool,
    pub available_to_musicians: bool,
}

/// A catalog snapshot returned to the operator UI.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub catalog_revision: String,
    pub sources: Vec<SourceInfo>,
}

/// An IPC error with a stable code and human-readable message.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
}
