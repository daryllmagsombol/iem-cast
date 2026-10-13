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
    /// HTTPS/WSS port to bind on the selected interface. Defaults to 8443 when omitted so existing
    /// callers remain valid.
    #[serde(default)]
    pub port: Option<u16>,
}

/// A local monitor output device offered to the operator.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
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
///
/// Carries the truthful configured metadata (`role`, `stereoPair`, `stereoSide`, `defaultPan`) from
/// the committed/prepared projection, so a valid configured map is never reported as hardcoded
/// mono. `available`/`available_to_musicians` retain the read-only physical/cast flags.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub source_id: String,
    pub physical_index: u16,
    pub label: String,
    /// Configured role: `inputChannel` or `masterLr`.
    pub role: String,
    pub available: bool,
    pub available_to_musicians: bool,
    /// Configured reciprocal stereo peer source id, when this source is linked.
    pub stereo_pair: Option<String>,
    /// Configured side (`left`/`right`) when this source is linked.
    pub stereo_side: Option<String>,
    /// Configured default pan/balance in `[-1.0, 1.0]`.
    pub default_pan: f32,
}

/// A catalog snapshot returned to the operator UI.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub catalog_revision: String,
    pub sources: Vec<SourceInfo>,
}

/// Which binding state a [`SourceSettings`] view describes.
///
/// The wire strings are exactly `saved`/`new`/`replacement`/`legacy`; the UI distinguishes an
/// unsaved preview from an installed profile using this, never by comparing revisions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BindingStatus {
    /// A committed profile that matches the selected device and verified layout.
    Saved,
    /// No committed profile yet; the prepared candidate would be a fresh binding.
    New,
    /// A committed profile for a different device/layout; installing it needs explicit consent.
    Replacement,
    /// A migrated legacy profile with no verified layout (unresolved until explicitly resolved).
    Legacy,
}

/// One per-source default pan/balance entry in a save payload.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePanDefaultDto {
    pub source_id: String,
    pub pan: f32,
}

/// One explicit left/right stereo link in a save payload.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLinkPair {
    pub left_source_id: String,
    pub right_source_id: String,
}

/// The source-settings view for the selected device.
///
/// `settings_revision` always denotes the **committed save base** (not a saved draft), so the
/// caller compares it against the revision it last saw. `durability_uncertain` is `true` only for
/// an installed view whose restart durability was not confirmed.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSettings {
    pub device_id: String,
    pub input_channels: u16,
    pub settings_revision: String,
    pub binding_status: BindingStatus,
    pub durability_uncertain: bool,
    pub catalog: CatalogSnapshot,
}

/// Sensible default host-setup values discovered by the desktop backend.
///
/// The frontend never hard-codes paths or interface names; it displays only what this reports and
/// keeps every field editable. A field is `None` when nothing discoverable exists, so the UI can
/// leave the input blank rather than showing a fabricated value.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDefaults {
    /// Preferred non-loopback interface name (`en0` when present), if any.
    pub interface_name: Option<String>,
    /// IPv4 address of the selected interface, if any.
    pub interface_ip: Option<String>,
    /// Absolute path to an existing `local-certs/cert.pem`, if discovered.
    pub certificate_path: Option<String>,
    /// Absolute path to an existing `local-certs/key.pem`, if discovered.
    pub key_path: Option<String>,
}

/// An IPC error with a stable code and human-readable message.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

/// Project a real `host-core` catalog snapshot into the IPC DTO.
///
/// This preserves the truthful configured role/pair/side/default metadata; nothing is inferred or
/// hardcoded. `authorized` maps to `available_to_musicians` and `available` mirrors physical
/// presence (a configured, committed source is present).
pub fn catalog_from_core(core: &host_core::contract::CatalogSnapshot) -> CatalogSnapshot {
    CatalogSnapshot {
        catalog_revision: core.catalog_revision.to_string(),
        sources: core
            .sources
            .iter()
            .map(|source| SourceInfo {
                source_id: source.source_id.to_string(),
                physical_index: source.physical_index,
                label: source.label.clone(),
                role: role_str(source.role),
                available: source.available,
                available_to_musicians: source.authorized,
                stereo_pair: source.stereo_pair.map(|peer| peer.to_string()),
                stereo_side: source.stereo_side.map(side_str),
                default_pan: source.default_pan,
            })
            .collect(),
    }
}

/// The wire string for a configured source role.
fn role_str(role: host_core::contract::SourceRole) -> String {
    match role {
        host_core::contract::SourceRole::InputChannel => "inputChannel".to_string(),
        host_core::contract::SourceRole::MasterLr => "masterLr".to_string(),
    }
}

/// The wire string for a configured stereo side.
fn side_str(side: host_core::contract::StereoSide) -> String {
    match side {
        host_core::contract::StereoSide::Left => "left".to_string(),
        host_core::contract::StereoSide::Right => "right".to_string(),
    }
}
