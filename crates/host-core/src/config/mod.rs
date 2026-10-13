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
//!   and its bytes are left exactly as they were. Current-version files are fully validated (pairs
//!   resolved, defaults complete, indices in bounds); sparse default vectors are normalized to
//!   center before that validation.
//! - `commit` is transactional and durable: it validates, writes a temp file in the same directory,
//!   fsyncs, atomically renames, then fsyncs the parent directory. A failure **before** the rename
//!   leaves memory, the file, and the revision untouched. If the rename succeeds but directory
//!   durability cannot be confirmed, the store reconciles memory to the installed revision and
//!   returns [`ConfigError::DurabilityUncertain`] — it never claims ordinary success or that the
//!   original file is unchanged.
//! - `bind` is the explicit stopped-host rebinding operation: a different device or layout mints
//!   fresh source ids and centered defaults. `verify_layout` is same-device verification and
//!   retains compatible ids/map/labels; an unresolved legacy configuration is preserved in memory
//!   for the UI but never written as an (invalid) current-version file until the operator resolves
//!   the sides.
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
    /// A filesystem read/write/flush/rename failure **before** the new file is installed. Memory,
    /// the file, and the revision are unchanged.
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
    /// The new file was installed but parent-directory durability could not be confirmed. Memory and
    /// revision were reconciled to the installed file; the caller must reload current settings
    /// before retrying. The original file is **not** guaranteed to be unchanged.
    #[error("config installed at revision {revision} but durability is unconfirmed: {source}")]
    DurabilityUncertain {
        /// The revision actually installed on disk.
        revision: u64,
        /// The underlying sync error.
        #[source]
        source: std::io::Error,
    },
    /// The monotonic settings revision is exhausted; no save is possible.
    #[error("settings revision is exhausted")]
    RevisionExhausted,
}

/// The outcome of an atomic write.
enum PersistOutcome {
    /// The new file was installed and the parent directory synced.
    Installed,
    /// The new file was installed (renamed into place) but the parent-directory sync failed.
    InstalledUncertain(std::io::Error),
}

/// A stage at which the atomic writer can be made to fail deterministically in tests. The variants
/// reachable before the rename leave the prior file untouched; the directory variants are only used
/// where the platform supports directory handles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WriteStage {
    CreateTemp,
    Write,
    SyncFile,
    Rename,
    #[cfg(unix)]
    OpenDir,
    #[cfg(unix)]
    SyncDir,
}

/// A transactional, device-bound configuration store.
#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    committed: ConfigV2,
    revision: u64,
    /// Deterministic fault injection; a private test-only seam with no production API.
    #[cfg(test)]
    fault: Option<WriteStage>,
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
                    #[cfg(test)]
                    fault: None,
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
                // Legacy loads are relaxed: unresolved pairs and unknown layouts are preserved for
                // the UI. Only decode-level integrity is required here.
                validate_legacy_load(&migrated)?;
                let revision = migrated.settings_revision.get();
                (migrated, revision)
            }
            2 => {
                let current: ConfigV2 =
                    serde_json::from_slice(&bytes).map_err(|_| ConfigError::Corrupt)?;
                let normalized = normalize_and_validate_v2(current)?;
                let revision = normalized.settings_revision.get();
                (normalized, revision)
            }
            other => return Err(ConfigError::Unsupported(other as u32)),
        };

        Ok(Self {
            path: path.to_path_buf(),
            committed,
            revision,
            #[cfg(test)]
            fault: None,
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
    /// Rejects a stale `expected_revision`, (once bound) a candidate that changes `device_id`, and
    /// (once the layout is verified) any change to the verified channel count or the
    /// source-id/physical-index binding. The candidate is validated, written atomically and
    /// durably, and only then replaces the in-memory committed value. A failure before the rename
    /// leaves memory and the revision unchanged.
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
            // The verified layout is immutable through an ordinary commit; a real layout change is
            // an explicit `bind`.
            if candidate.input_channels != self.committed.input_channels
                || binding_pairs(&candidate.channel_map)
                    != binding_pairs(&self.committed.channel_map)
            {
                return Err(ConfigError::Corrupt);
            }
        }

        // Checked revision advancement *before* any filesystem work.
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(ConfigError::RevisionExhausted)?;

        candidate.version = CONFIG_VERSION;
        validate_config(&candidate)?;
        candidate.settings_revision = SettingsRevision(next_revision);

        let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| ConfigError::Corrupt)?;
        self.persist_candidate(candidate, next_revision, &bytes)
    }

    /// Explicitly (re)bind to a verified capture device and layout.
    ///
    /// This is the stopped-host rebinding operation. A different device, or a layout change, mints
    /// **fresh** source ids and centered defaults. A same-device verification (see
    /// [`verify_layout`](Self::verify_layout)) is expressed by calling `bind` where the existing map
    /// is already compatible, in which case ids/map/labels are retained.
    pub fn bind(
        &mut self,
        device_id: &str,
        verified_channels: u16,
        expected_revision: u64,
    ) -> Result<ConfigV2, ConfigError> {
        self.apply_binding(device_id, verified_channels, expected_revision, true)
    }

    /// Verify the existing binding against a same-device capture layout, retaining compatible
    /// ids/map/labels (including unresolved legacy links).
    ///
    /// Never mints fresh ids. A different device is a [`DeviceMismatch`](ConfigError::DeviceMismatch);
    /// an incompatible map is [`Corrupt`](ConfigError::Corrupt) so the caller must explicitly `bind`.
    /// A resolved configuration is persisted and advances the revision. An unresolved legacy
    /// configuration is adopted **in memory only** so the operator can choose sides or unlink; it is
    /// never written as an invalid current-version file.
    pub fn verify_layout(
        &mut self,
        device_id: &str,
        verified_channels: u16,
        expected_revision: u64,
    ) -> Result<ConfigV2, ConfigError> {
        self.apply_binding(device_id, verified_channels, expected_revision, false)
    }

    fn apply_binding(
        &mut self,
        device_id: &str,
        verified_channels: u16,
        expected_revision: u64,
        explicit: bool,
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

        let same_uid = self.committed.device_id.as_deref() == Some(device_id);
        if !same_uid && !explicit {
            return Err(ConfigError::DeviceMismatch {
                bound: self.committed.device_id.clone().unwrap_or_default(),
                requested: device_id.to_string(),
            });
        }

        let compatible = map_compatible(
            &self.committed.channel_map,
            self.committed.input_channels,
            verified_channels,
        );
        let retain = same_uid && compatible;

        // Same-device verification with an incompatible map: the operator must explicitly rebind.
        if !retain && !explicit {
            return Err(ConfigError::Corrupt);
        }

        // A redundant verification of an already-valid binding is a no-op (no write, no revision).
        if retain
            && self.committed.input_channels == Some(verified_channels)
            && validate_config(&self.committed).is_ok()
        {
            return Ok(self.committed.clone());
        }

        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(ConfigError::RevisionExhausted)?;

        let (channel_map, source_defaults, labels) = if retain {
            (
                self.committed.channel_map.clone(),
                self.committed.source_defaults.clone(),
                self.committed.labels.clone(),
            )
        } else {
            let map = fresh_channel_map(verified_channels);
            let defaults = centered_defaults(&map);
            (map, defaults, Vec::new())
        };

        let candidate = ConfigV2 {
            version: CONFIG_VERSION,
            device_id: Some(device_id.to_string()),
            selected_interface: self.committed.selected_interface.clone(),
            input_channels: Some(verified_channels),
            settings_revision: SettingsRevision(next_revision),
            channel_map,
            labels,
            source_defaults,
        };

        if validate_config(&candidate).is_ok() {
            let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| ConfigError::Corrupt)?;
            self.persist_candidate(candidate, next_revision, &bytes)
        } else if retain {
            // Unresolved legacy links: preserve them for the UI in memory, but never write an
            // invalid current-version file to disk. The revision does not advance because nothing
            // durable changed.
            let mut adopted = candidate;
            adopted.settings_revision = SettingsRevision(self.revision);
            self.committed = adopted.clone();
            Ok(adopted)
        } else {
            Err(ConfigError::Corrupt)
        }
    }

    /// Write `candidate` atomically, reconciling memory only on success or durable uncertainty.
    fn persist_candidate(
        &mut self,
        candidate: ConfigV2,
        next_revision: u64,
        bytes: &[u8],
    ) -> Result<ConfigV2, ConfigError> {
        match self.persist(bytes)? {
            PersistOutcome::Installed => {
                self.committed = candidate.clone();
                self.revision = next_revision;
                Ok(candidate)
            }
            PersistOutcome::InstalledUncertain(source) => {
                // The new file is installed: adopt the installed revision, then report uncertainty.
                self.committed = candidate;
                self.revision = next_revision;
                Err(ConfigError::DurabilityUncertain {
                    revision: next_revision,
                    source,
                })
            }
        }
    }

    fn persist(&self, bytes: &[u8]) -> Result<PersistOutcome, ConfigError> {
        #[cfg(test)]
        let fault = self.fault;
        #[cfg(not(test))]
        let fault = None;
        write_atomic(&self.path, bytes, fault)
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

/// The sorted `(physical_index, source_id)` binding of a map, used to detect layout changes.
fn binding_pairs(map: &[ChannelMapEntry]) -> Vec<(u16, SourceId)> {
    let mut pairs: Vec<(u16, SourceId)> =
        map.iter().map(|e| (e.physical_index, e.source_id)).collect();
    pairs.sort_unstable();
    pairs
}

/// Whether an existing map is compatible with a verified channel layout: same verified count (when
/// one was recorded), the same number of entries, and every physical index in bounds.
fn map_compatible(
    map: &[ChannelMapEntry],
    committed_channels: Option<u16>,
    verified_channels: u16,
) -> bool {
    if let Some(recorded) = committed_channels {
        if recorded != verified_channels {
            return false;
        }
    }
    map.len() == verified_channels as usize
        && map.iter().all(|entry| entry.physical_index < verified_channels)
}

/// Full validation of a current-version config: a verified supported channel count for any
/// populated device-bound state, resolved reciprocal pairs, in-bounds indices, complete finite
/// defaults, and equal pans within a pair.
///
/// Only an **empty, unbound** base state may omit the layout (`device_id: None` +
/// `input_channels: None` + empty map). A populated map without a verified count, a bound device
/// without a count, or a count without a device are inconsistent bindings and rejected. This is the
/// single gate used by both `open` (through [`normalize_and_validate_v2`]) and `commit`.
pub fn validate_config(config: &ConfigV2) -> Result<(), ConfigError> {
    validate_binding_layout(config)?;
    validate_source_map(&config.channel_map)?;
    validate_source_defaults(&config.channel_map, &config.source_defaults)
}

/// Require a verified supported count for populated/device-bound canonical states.
///
/// Allowed combinations only:
/// - `(None, None)` with an empty map — the fresh unbound base state.
/// - `(Some(device), Some(channels))` — a bound, verified layout, fully bounds-checked.
///
/// Everything else (populated map with no count, bound device with no count, a count without a
/// device) is an inconsistent binding.
fn validate_binding_layout(config: &ConfigV2) -> Result<(), ConfigError> {
    match (config.device_id.as_deref(), config.input_channels) {
        (None, None) => {
            if !config.channel_map.is_empty() {
                return Err(ConfigError::Corrupt);
            }
            Ok(())
        }
        (Some(_), Some(channels)) => {
            if channels == 0 || channels as usize > MAX_SOURCES {
                return Err(ConfigError::Corrupt);
            }
            if config.channel_map.len() != channels as usize {
                return Err(ConfigError::Corrupt);
            }
            if config
                .channel_map
                .iter()
                .any(|entry| entry.physical_index >= channels)
            {
                return Err(ConfigError::Corrupt);
            }
            Ok(())
        }
        _ => Err(ConfigError::Corrupt),
    }
}

/// Normalize a loaded current-version config then fully validate it.
///
/// Sparse default vectors are allowed on load: a mapped source with no record is centered. Any
/// duplicate/unknown default, non-finite/out-of-range pan, unresolved pair, or out-of-bounds index
/// is [`Corrupt`](ConfigError::Corrupt). Canonical save vectors remain complete.
fn normalize_and_validate_v2(mut config: ConfigV2) -> Result<ConfigV2, ConfigError> {
    let mut seen = HashSet::new();
    for default in &config.source_defaults {
        if !seen.insert(default.source_id)
            || !config
                .channel_map
                .iter()
                .any(|entry| entry.source_id == default.source_id)
        {
            return Err(ConfigError::Corrupt);
        }
    }
    let complete = config
        .channel_map
        .iter()
        .map(|entry| SourcePanDefault {
            source_id: entry.source_id,
            pan: config
                .source_defaults
                .iter()
                .find(|default| default.source_id == entry.source_id)
                .map(|default| default.pan)
                .unwrap_or(0.0),
        })
        .collect();
    config.source_defaults = complete;
    validate_config(&config)?;
    Ok(config)
}

/// Decode-level validation for a migrated legacy config: unique ids/indices and finite in-range
/// default values. Pair resolution and layout bounds are intentionally not required.
fn validate_legacy_load(config: &ConfigV2) -> Result<(), ConfigError> {
    let mut ids = HashSet::new();
    let mut indices = HashSet::new();
    for entry in &config.channel_map {
        if !ids.insert(entry.source_id) || !indices.insert(entry.physical_index) {
            return Err(ConfigError::Corrupt);
        }
    }
    validate_source_defaults_values(&config.channel_map, &config.source_defaults)
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
    validate_source_defaults_values(map, defaults)?;
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

fn validate_source_defaults_values(
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
/// the right member [`StereoSide::Right`]. A member whose link **membership or side** changed has
/// its default reset to center, so a reversal recenters both members. Nothing is inferred: an
/// unknown id, self-pair, or a source appearing in two links is rejected.
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

    // A member is "affected" when its peer **or** its side changed (a reversal keeps the peer but
    // swaps sides), so a reversal recenters both members. Unrelated defaults are untouched.
    let changed: HashSet<SourceId> = map
        .iter()
        .zip(linked.iter())
        .filter(|(before, after)| {
            before.stereo_pair != after.stereo_pair || before.stereo_side != after.stereo_side
        })
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

/// Build a synthetic I/O error for the deterministic test seam.
fn injected(stage: &str) -> std::io::Error {
    std::io::Error::other(format!("injected write failure at {stage}"))
}

/// Write `bytes` to `path` atomically and durably.
///
/// The parent directory is opened **before** anything is replaced, so a directory-open failure is a
/// pre-install failure. The temp file is written and fsynced in the same directory, renamed into
/// place, and then the already-open directory handle is synced. A failure before the rename leaves
/// the prior file untouched; a failure syncing the directory after a successful rename reports
/// [`PersistOutcome::InstalledUncertain`] because the new file is installed but durability is not
/// confirmed.
fn write_atomic(
    path: &Path,
    bytes: &[u8],
    fault: Option<WriteStage>,
) -> Result<PersistOutcome, ConfigError> {
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config path has no file name",
        ))
    })?;

    // 1. Open the parent directory before replacing anything (pre-install failure).
    #[cfg(unix)]
    let dir_handle = {
        if fault == Some(WriteStage::OpenDir) {
            return Err(ConfigError::Io(injected("open dir")));
        }
        fs::File::open(directory).map_err(ConfigError::Io)?
    };

    let temp = directory.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    ));

    // 2. Write + fsync the temp file (pre-install failure; temp removed on error).
    let write_result = (|| -> std::io::Result<()> {
        if fault == Some(WriteStage::CreateTemp) {
            return Err(injected("create temp"));
        }
        let mut file = fs::File::create(&temp)?;
        if fault == Some(WriteStage::Write) {
            return Err(injected("write"));
        }
        file.write_all(bytes)?;
        file.flush()?;
        if fault == Some(WriteStage::SyncFile) {
            return Err(injected("sync file"));
        }
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Io(error));
    }

    // 3. Atomic replacement (pre-install failure; temp removed on error).
    if fault == Some(WriteStage::Rename) {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Io(injected("rename")));
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Io(error));
    }

    // 4. Post-install directory sync. A failure here cannot promise the prior file is unchanged.
    #[cfg(unix)]
    {
        if fault == Some(WriteStage::SyncDir) {
            return Ok(PersistOutcome::InstalledUncertain(injected("sync dir")));
        }
        match dir_handle.sync_all() {
            Ok(()) => Ok(PersistOutcome::Installed),
            Err(error) => Ok(PersistOutcome::InstalledUncertain(error)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = fault;
        Ok(PersistOutcome::Installed)
    }
}

#[cfg(test)]
mod tests;
