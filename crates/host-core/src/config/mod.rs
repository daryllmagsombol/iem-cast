//! Transactional, device-bound configuration persistence (single profile, no database).
//!
//! The store is the operator's durable source-settings boundary: one app-local file holding
//! [`ConfigV2`] (mapping, labels, defaults, and the verified device binding). It is deliberately
//! **not** on the audio path and holds only non-secret operator settings — never credentials,
//! epochs, nonces, arm/output state, or listening permission.
//!
//! Semantics that matter:
//!
//! - `open` loads and migrates but never writes. A missing file is a fresh **unbound** empty
//!   config, not a fabricated device. A corrupt or future-versioned file is surfaced as an error
//!   and its bytes are left exactly as they were.
//! - `commit` is transactional and durable: it validates, writes a temp file in the same directory,
//!   fsyncs, renames atomically, then fsyncs the parent directory. The in-memory committed value and
//!   revision advance **only after** the rename succeeds; an I/O failure leaves both untouched.
//! - `bind` is the explicit stopped-host rebinding operation; it is distinct from an accidental
//!   device change arriving through `commit`.
//! - A legacy v1 file migrates to v2 preserving ids, mapping, and labels, with center defaults and
//!   revision `0`. Unresolved legacy pairs are preserved for the UI but cannot be saved until the
//!   operator chooses an explicit side or unlinks — no side is ever inferred.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::contract::{
    ChannelMapEntry, Config, ConfigV2, SourcePanDefault, SourceRole, StereoSide,
};
use crate::ids::{SettingsRevision, SourceId, MAX_SOURCES};

/// The current persisted configuration version.
pub const CONFIG_VERSION: u32 = 2;

/// A configuration persistence failure. `Io` sources the underlying error; the others are explicit
/// and must never be collapsed into a silent fallback.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A filesystem read/write/flush/rename failure.
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not decodable, or the configuration is structurally invalid.
    #[error("config file is corrupt")]
    Corrupt,
    /// The file declares a version this build does not understand (never overwritten).
    #[error("unsupported config version {0}")]
    Unsupported(u32),
    /// The caller's expected revision does not match the committed revision.
    #[error("revision conflict: expected {expected}, found {found}")]
    RevisionConflict { expected: u64, found: u64 },
    /// A commit attempted to change the bound device; use `bind` for an explicit rebind.
    #[error("device mismatch: bound {bound}, requested {requested}")]
    DeviceMismatch { bound: String, requested: String },
}

/// A transactional, device-bound configuration store.
#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    committed: ConfigV2,
    revision: u64,
}

impl ConfigStore {
    /// Open (or initialize) the store at `path`: load, migrate, and validate.
    ///
    /// A missing file yields a fresh unbound empty config and is **not** created. A corrupt file or
    /// an unsupported version returns an error and leaves the file bytes untouched.
    pub fn open(path: &Path) -> Result<Self, ConfigError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    path: path.to_path_buf(),
                    committed: empty_config(),
                    revision: 0,
                });
            }
            Err(error) => return Err(ConfigError::Io(error)),
        };

        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| ConfigError::Corrupt)?;
        let version = value
            .get("version")
            .and_then(|v| v.as_u64())
            .ok_or(ConfigError::Corrupt)?;

        let (committed, revision) = match version {
            1 => {
                let legacy: Config =
                    serde_json::from_slice(&bytes).map_err(|_| ConfigError::Corrupt)?;
                let migrated = migrate_v1(legacy)?;
                let revision = migrated.settings_revision.get();
                (migrated, revision)
            }
            2 => {
                let current: ConfigV2 =
                    serde_json::from_slice(&bytes).map_err(|_| ConfigError::Corrupt)?;
                let revision = current.settings_revision.get();
                (current, revision)
            }
            other => return Err(ConfigError::Unsupported(other as u32)),
        };

        // Decode-level validation: a duplicate id or index is structurally corrupt. Pair resolution
        // is deliberately NOT required here, so an unresolved legacy pair still loads for the UI.
        validate_loadable(&committed)?;

        Ok(Self {
            path: path.to_path_buf(),
            committed,
            revision,
        })
    }

    /// The bound CPAL device id, if any.
    pub fn binding(&self) -> Option<&str> {
        self.committed.device_id.as_deref()
    }

    /// The currently committed configuration.
    pub fn committed(&self) -> &ConfigV2 {
        &self.committed
    }

    /// The committed settings revision (the truth the UI and commands compare against).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the committed map carries an unresolved pair (a peer or side without a complete,
    /// reciprocal, opposite-side link). Such a load is preserved for the UI, but cannot be saved
    /// until the operator chooses an explicit side or unlinks.
    pub fn has_unresolved_pairs(&self) -> bool {
        let map = &self.committed.channel_map;
        let has_marker = map
            .iter()
            .any(|e| e.stereo_pair.is_some() || e.stereo_side.is_some());
        has_marker && validate_source_map(map).is_err()
    }

    /// Commit a candidate configuration, advancing the settings revision.
    ///
    /// Rejects a stale `expected_revision` and (once bound) a candidate that changes `device_id`.
    /// The candidate is validated, written atomically and durably, and only then replaces the
    /// in-memory committed value. On any I/O failure the memory and revision are unchanged.
    pub fn commit(
        &mut self,
        expected_revision: u64,
        mut candidate: ConfigV2,
    ) -> Result<ConfigV2, ConfigError> {
        if expected_revision != self.revision {
            return Err(ConfigError::RevisionConflict {
                expected: expected_revision,
                found: self.revision,
            });
        }
        if let Some(bound) = self.committed.device_id.as_deref() {
            if candidate.device_id.as_deref() != Some(bound) {
                return Err(ConfigError::DeviceMismatch {
                    bound: bound.to_string(),
                    requested: candidate.device_id.clone().unwrap_or_default(),
                });
            }
        }

        candidate.version = CONFIG_VERSION;
        validate_config(&candidate)?;

        // The store owns revision advancement, so memory and disk cannot drift.
        candidate.settings_revision = SettingsRevision(self.revision + 1);
        let next_revision = candidate.settings_revision.get();

        let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| ConfigError::Corrupt)?;
        write_atomic(&self.path, &bytes)?;

        self.committed = candidate.clone();
        self.revision = next_revision;
        Ok(candidate)
    }

    /// Explicitly (re)bind to a verified capture device and layout.
    ///
    /// This is the stopped-host rebinding operation and is distinct from an accidental device change
    /// through `commit`. A different device or layout mints **fresh** source ids and centered
    /// defaults rather than inheriting the previous map; the same device and layout retain their
    /// existing ids and map across restarts and pair changes.
    pub fn bind(
        &mut self,
        device_id: &str,
        verified_channels: u16,
        expected_revision: u64,
    ) -> Result<ConfigV2, ConfigError> {
        if expected_revision != self.revision {
            return Err(ConfigError::RevisionConflict {
                expected: expected_revision,
                found: self.revision,
            });
        }
        if verified_channels == 0 || verified_channels as usize > MAX_SOURCES {
            return Err(ConfigError::Corrupt);
        }

        let same_binding = self.committed.device_id.as_deref() == Some(device_id)
            && self.committed.input_channels == Some(verified_channels);

        let (channel_map, source_defaults) = if same_binding {
            (
                self.committed.channel_map.clone(),
                self.committed.source_defaults.clone(),
            )
        } else {
            let map = fresh_channel_map(verified_channels);
            let defaults = centered_defaults(&map);
            (map, defaults)
        };

        let candidate = ConfigV2 {
            version: CONFIG_VERSION,
            device_id: Some(device_id.to_string()),
            selected_interface: self.committed.selected_interface.clone(),
            input_channels: Some(verified_channels),
            settings_revision: SettingsRevision(self.revision + 1),
            channel_map,
            labels: self.committed.labels.clone(),
            source_defaults,
        };
        validate_config(&candidate)?;

        let next_revision = candidate.settings_revision.get();
        let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| ConfigError::Corrupt)?;
        write_atomic(&self.path, &bytes)?;

        self.committed = candidate.clone();
        self.revision = next_revision;
        Ok(candidate)
    }
}

fn empty_config() -> ConfigV2 {
    ConfigV2 {
        version: CONFIG_VERSION,
        device_id: None,
        selected_interface: None,
        input_channels: None,
        settings_revision: SettingsRevision(0),
        channel_map: Vec::new(),
        labels: Vec::new(),
        source_defaults: Vec::new(),
    }
}

/// Migrate a legacy v1 config to v2, preserving ids, mapping, and labels.
fn migrate_v1(legacy: Config) -> Result<ConfigV2, ConfigError> {
    if legacy.version != 1 {
        return Err(ConfigError::Unsupported(legacy.version));
    }
    let source_defaults = centered_defaults(&legacy.channel_map);
    Ok(ConfigV2 {
        version: CONFIG_VERSION,
        device_id: legacy.device_id,
        selected_interface: legacy.selected_interface,
        // v1 carried no independently verified channel count; migrate as unknown, never invented.
        input_channels: None,
        settings_revision: SettingsRevision(0),
        channel_map: legacy.channel_map,
        labels: legacy.labels,
        source_defaults,
    })
}

/// A fresh, mono input-channel map with new random source ids.
///
/// Discovery maps every physical channel as a mono `InputChannel`; no pair is inferred from
/// hardware. A new device binding therefore starts centered and unpaired.
pub fn fresh_channel_map(channels: u16) -> Vec<ChannelMapEntry> {
    (0..channels)
        .map(|index| ChannelMapEntry {
            physical_index: index,
            source_id: SourceId(Uuid::new_v4()),
            stereo_pair: None,
            stereo_side: None,
            role: SourceRole::InputChannel,
        })
        .collect()
}

/// Centered defaults, one per mapped source, in map order.
pub fn centered_defaults(map: &[ChannelMapEntry]) -> Vec<SourcePanDefault> {
    map.iter()
        .map(|entry| SourcePanDefault {
            source_id: entry.source_id,
            pan: 0.0,
        })
        .collect()
}

/// Decode-level validation that must hold even for an unresolved legacy load.
fn validate_loadable(config: &ConfigV2) -> Result<(), ConfigError> {
    let mut ids = HashSet::new();
    let mut indices = HashSet::new();
    for entry in &config.channel_map {
        if !ids.insert(entry.source_id) || !indices.insert(entry.physical_index) {
            return Err(ConfigError::Corrupt);
        }
    }
    validate_source_defaults_values(config)?;
    Ok(())
}

/// Full configuration validation, applied before any save.
pub fn validate_config(config: &ConfigV2) -> Result<(), ConfigError> {
    validate_source_map(&config.channel_map)?;
    if let Some(channels) = config.input_channels {
        if config.channel_map.len() != channels as usize {
            return Err(ConfigError::Corrupt);
        }
    }
    validate_source_defaults(&config.channel_map, &config.source_defaults)
}

/// Validate a configured channel map: unique ids and indices, reciprocal pairs with opposite sides
/// and matching roles, and no dangling, self, or overlapping links.
pub fn validate_source_map(map: &[ChannelMapEntry]) -> Result<(), ConfigError> {
    let mut ids = HashSet::new();
    let mut indices = HashSet::new();
    for entry in map {
        if !ids.insert(entry.source_id) || !indices.insert(entry.physical_index) {
            return Err(ConfigError::Corrupt);
        }
    }

    for entry in map {
        match (entry.stereo_pair, entry.stereo_side) {
            // An explicit complete link.
            (Some(peer), Some(side)) => {
                if peer == entry.source_id {
                    return Err(ConfigError::Corrupt);
                }
                let peer_entry = map
                    .iter()
                    .find(|candidate| candidate.source_id == peer)
                    .ok_or(ConfigError::Corrupt)?;
                if peer_entry.stereo_pair != Some(entry.source_id) {
                    return Err(ConfigError::Corrupt);
                }
                match peer_entry.stereo_side {
                    Some(peer_side) if peer_side != side => {}
                    _ => return Err(ConfigError::Corrupt),
                }
                if peer_entry.role != entry.role {
                    return Err(ConfigError::Corrupt);
                }
            }
            // Unpaired.
            (None, None) => {}
            // A dangling peer or a side without a pair is unresolved/invalid.
            _ => return Err(ConfigError::Corrupt),
        }
    }
    Ok(())
}

/// Validate source defaults: exactly one per mapped source, finite, within `[-1, 1]`, and equal for
/// the two members of any explicit pair.
pub fn validate_source_defaults(
    map: &[ChannelMapEntry],
    defaults: &[SourcePanDefault],
) -> Result<(), ConfigError> {
    validate_source_defaults_values_raw(map, defaults)?;
    if defaults.len() != map.len() {
        return Err(ConfigError::Corrupt);
    }
    for entry in map {
        if let Some(peer) = entry.stereo_pair {
            let left = defaults
                .iter()
                .find(|d| d.source_id == entry.source_id)
                .map(|d| d.pan);
            let right = defaults.iter().find(|d| d.source_id == peer).map(|d| d.pan);
            if left != right {
                return Err(ConfigError::Corrupt);
            }
        }
    }
    Ok(())
}

fn validate_source_defaults_values(config: &ConfigV2) -> Result<(), ConfigError> {
    validate_source_defaults_values_raw(&config.channel_map, &config.source_defaults)
}

fn validate_source_defaults_values_raw(
    map: &[ChannelMapEntry],
    defaults: &[SourcePanDefault],
) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();
    for default in defaults {
        if !seen.insert(default.source_id) {
            return Err(ConfigError::Corrupt);
        }
        if !default.pan.is_finite() || !(-1.0..=1.0).contains(&default.pan) {
            return Err(ConfigError::Corrupt);
        }
        if !map.iter().any(|entry| entry.source_id == default.source_id) {
            return Err(ConfigError::Corrupt);
        }
    }
    Ok(())
}

/// Apply a complete explicit link set, replacing all existing pairs.
///
/// Each pair is `(left_source_id, right_source_id)`; the left member gets [`StereoSide::Left`] and
/// the right member [`StereoSide::Right`]. Affected members' defaults are reset to center. Nothing
/// is inferred: an unknown id, self-pair, or a source appearing in two links is rejected.
pub fn apply_source_links(
    map: &[ChannelMapEntry],
    defaults: &[SourcePanDefault],
    pairs: &[(SourceId, SourceId)],
) -> Result<(Vec<ChannelMapEntry>, Vec<SourcePanDefault>), ConfigError> {
    let mut linked = map
        .iter()
        .map(|entry| ChannelMapEntry {
            stereo_pair: None,
            stereo_side: None,
            ..*entry
        })
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();

    for (left, right) in pairs {
        if left == right {
            return Err(ConfigError::Corrupt);
        }
        if !seen.insert(*left) || !seen.insert(*right) {
            return Err(ConfigError::Corrupt);
        }
        {
            let entry = linked
                .iter_mut()
                .find(|e| e.source_id == *left)
                .ok_or(ConfigError::Corrupt)?;
            entry.stereo_side = Some(StereoSide::Left);
            entry.stereo_pair = Some(*right);
        }
        {
            let entry = linked
                .iter_mut()
                .find(|e| e.source_id == *right)
                .ok_or(ConfigError::Corrupt)?;
            entry.stereo_side = Some(StereoSide::Right);
            entry.stereo_pair = Some(*left);
        }
    }

    validate_source_map(&linked)?;

    // Any member whose link membership or peer changed has its default reset to center; a routing
    // change never resets gains/mutes, only the affected defaults.
    let changed: HashSet<SourceId> = map
        .iter()
        .zip(linked.iter())
        .filter(|(before, after)| before.stereo_pair != after.stereo_pair)
        .map(|(_, after)| after.source_id)
        .collect();

    let recentered = defaults
        .iter()
        .map(|default| SourcePanDefault {
            source_id: default.source_id,
            pan: if changed.contains(&default.source_id) {
                0.0
            } else {
                default.pan
            },
        })
        .collect::<Vec<_>>();
    validate_source_defaults(&linked, &recentered)?;
    Ok((linked, recentered))
}

/// Write `bytes` to `path` atomically and durably: temp file in the same directory, flush + fsync,
/// atomic rename, then a parent-directory sync. A failure never leaves a partial file in place.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config path has no file name",
        )))?;
    let temp = directory.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    ));

    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    };

    if let Err(error) = write() {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Io(error));
    }

    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Io(error));
    }

    // Best-effort parent directory sync so the rename itself is durable on platforms that need it.
    #[cfg(unix)]
    {
        if let Ok(dir) = fs::File::open(directory) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
