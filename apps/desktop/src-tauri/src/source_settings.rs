//! Native source-settings manager.
//!
//! This is the desktop shell's settings boundary. It owns the one committed [`ConfigStore`], a
//! single **pending** prepared binding (so preview UUIDs stay stable across read -> save -> start),
//! and the small committed-projection cache a fresh phone connection reads. It never touches the
//! audio path and never opens a stream during a read.
//!
//! Core (`host-core`) owns identity continuity only. This module owns the composition policy:
//! resolving the **actual** verified capture identity, lifecycle/stopped-host enforcement, explicit
//! replacement consent, and reporting the durability-uncertain outcome truthfully.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use host_core::config::{
    apply_source_links, validate_config, validate_source_defaults, ConfigError, ConfigStore,
};
use host_core::contract::{CatalogSnapshot, ConfigV2, SourcePanDefault};
use host_core::host::catalog_from_config;
use host_core::ids::{CatalogRevision, SourceId};

use crate::bridge::{
    BindingStatus, CatalogSnapshot as BridgeCatalogSnapshot, IpcError, SourceLinkPair,
    SourcePanDefaultDto, SourceSettings,
};

/// The fixed operator capture rate used to verify a device layout.
///
/// This matches the desktop `start_host` request rate so setup verification and startup agree on
/// the same layout.
pub const SAMPLE_RATE_HZ: u32 = 48_000;

/// Resolve a device id to its stable capture UID and verified selected channel count.
///
/// Production uses the real CPAL UID + selected capture layout (no stream opened). Tests inject a
/// fake so no hardware or real user data is involved.
pub trait CaptureIdentity: Send + Sync + 'static {
    /// The `(uid, channels)` the stream would actually open for `device_id` at `sample_rate_hz`.
    fn verified(&self, device_id: &str, sample_rate_hz: u32) -> Result<(String, u16), String>;
}

/// The production identity resolver over `host-core`'s real capture backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCaptureIdentity;

impl CaptureIdentity for SystemCaptureIdentity {
    fn verified(&self, device_id: &str, sample_rate_hz: u32) -> Result<(String, u16), String> {
        host_core::capture::verified_capture_identity(device_id, sample_rate_hz)
            .map_err(|error| error.to_string())
    }
}

/// The one cached preview binding: the exact prepared candidate and the committed base it was
/// prepared against. It is reused for the same `(uid, channels, base)` so preview UUIDs never churn.
pub struct PendingBinding {
    /// The stable UID the candidate was prepared for.
    pub device_id: String,
    /// The verified channel count the candidate was prepared for.
    pub input_channels: u16,
    /// The committed revision the candidate was prepared against.
    pub base_revision: u64,
    /// The exact prepared candidate (ids/map/labels/defaults).
    pub candidate: ConfigV2,
}

/// The native settings manager. Fields are `pub(crate)` so the command layer and unit tests can
/// drive it; there is no public test-only API.
pub struct SettingsManager {
    pub(crate) path: PathBuf,
    pub(crate) store: Option<ConfigStore>,
    pub(crate) load_error: Option<String>,
    pub(crate) identity: Arc<dyn CaptureIdentity>,
    pub(crate) pending: Option<PendingBinding>,
    /// Committed-projection cache read by the per-connection provider. It holds ONLY catalog data;
    /// the provider closure never captures the lifecycle/settings mutex.
    pub(crate) projection: Arc<RwLock<CatalogSnapshot>>,
    pub(crate) durability_uncertain: bool,
}

/// Tauri-managed handle to the native settings manager.
pub struct SourceSettingsState(pub Mutex<SettingsManager>);

impl SourceSettingsState {
    /// Wrap an already-built manager.
    pub fn new(manager: SettingsManager) -> Self {
        Self(Mutex::new(manager))
    }

    /// Open the fixed settings file with the given identity resolver.
    ///
    /// A missing file is a fresh unbound profile. A corrupt or unreadable file is retained as a
    /// visible error state (never overwritten with an empty fallback) while the app still loads.
    pub fn open(path: impl Into<PathBuf>, identity: Arc<dyn CaptureIdentity>) -> Self {
        Self::new(SettingsManager::open(path, identity))
    }

    /// Lock the manager, surfacing a poisoned lock as a typed IPC error.
    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, SettingsManager>, IpcError> {
        self.0
            .lock()
            .map_err(|_| ipc("SOURCE_SETTINGS_STATE_POISONED", "source settings state is unavailable"))
    }
}

impl SettingsManager {
    /// Open the store at `path`, retaining a visible error state on a corrupt/unreadable file.
    pub fn open(path: impl Into<PathBuf>, identity: Arc<dyn CaptureIdentity>) -> Self {
        let path = path.into();
        let (store, load_error) = match ConfigStore::open(&path) {
            Ok(store) => (Some(store), None),
            // Never overwrite a corrupt file with an empty fallback; surface it instead.
            Err(error) => (None, Some(error.to_string())),
        };
        let projection = Arc::new(RwLock::new(projection_for(store.as_ref())));
        Self {
            path,
            store,
            load_error,
            identity,
            pending: None,
            projection,
            durability_uncertain: false,
        }
    }

    /// The fixed settings file path (for diagnostics/tests).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the store is currently unreadable, with the visible reason.
    pub fn unreadable(&self) -> Option<&str> {
        if self.store.is_none() {
            Some(
                self.load_error
                    .as_deref()
                    .unwrap_or("source settings are unreadable"),
            )
        } else {
            None
        }
    }

    /// The committed-projection cache. Holds catalog data only.
    pub fn projection(&self) -> Arc<RwLock<CatalogSnapshot>> {
        Arc::clone(&self.projection)
    }

    fn require_store(&self) -> Result<&ConfigStore, IpcError> {
        self.store
            .as_ref()
            .ok_or_else(|| unreadable_error(self.load_error.as_deref().unwrap_or("source settings are unreadable")))
    }

    /// Prepare or reuse the pending candidate for `(uid, channels)` against the current base.
    fn ensure_pending(&mut self, uid: &str, channels: u16) -> Result<ConfigV2, IpcError> {
        let base = self.require_store()?.revision();
        if let Some(existing) = self.pending.as_ref() {
            if existing.device_id == uid
                && existing.input_channels == channels
                && existing.base_revision == base
            {
                return Ok(existing.candidate.clone());
            }
        }
        // A different device/layout (or base) invalidates the prior preview and mints a fresh one.
        let candidate = self
            .require_store()?
            .prepare_binding(uid, channels)
            .map_err(|error| ipc("SOURCE_SETTINGS_PREPARE_FAILED", error.to_string()))?;
        self.pending = Some(PendingBinding {
            device_id: uid.to_string(),
            input_channels: channels,
            base_revision: base,
            candidate: candidate.clone(),
        });
        Ok(candidate)
    }

    /// Read the source-settings view for the selected device. Never writes.
    pub fn read_view(&mut self, device_id: &str) -> Result<SourceSettings, IpcError> {
        self.require_store()?;
        let (uid, channels) = self.verified_identity(device_id)?;
        let prepared = self.ensure_pending(&uid, channels)?;
        self.view(&uid, channels, &prepared)
    }

    /// Save a complete default-pan vector for the selected device.
    ///
    /// A compatible saved binding may be updated while the host runs; a new/legacy/replacement
    /// binding requires a stopped host, and a replacement additionally requires explicit consent.
    pub fn save_defaults(
        &mut self,
        host_running: bool,
        device_id: &str,
        base_revision: &str,
        defaults: &[SourcePanDefaultDto],
        replace_binding: bool,
    ) -> Result<SourceSettings, IpcError> {
        self.require_store()?;
        let base = parse_revision(base_revision)?;
        let (uid, channels) = self.verified_identity(device_id)?;
        let prepared = self.ensure_pending(&uid, channels)?;
        let status = self.status_for(base, &uid, channels, &prepared)?;
        if status == BindingStatus::Replacement && !replace_binding {
            return Err(consent_required());
        }
        if status != BindingStatus::Saved && host_running {
            return Err(host_running_error("defaults"));
        }

        let mut candidate = prepared;
        candidate.source_defaults = parse_defaults(defaults)?;
        validate_source_defaults(&candidate.channel_map, &candidate.source_defaults)
            .map_err(|_| invalid("the submitted defaults are not a complete, pair-equal set"))?;

        self.commit(base, candidate, status)?;
        self.saved_view(&uid, channels)
    }

    /// Save a complete explicit link set for the selected device.
    ///
    /// Requires a stopped host and resets affected defaults to center. A replacement binding
    /// additionally needs explicit consent.
    pub fn save_links(
        &mut self,
        host_running: bool,
        device_id: &str,
        base_revision: &str,
        pairs: &[SourceLinkPair],
        replace_binding: bool,
    ) -> Result<SourceSettings, IpcError> {
        self.require_store()?;
        if host_running {
            return Err(host_running_error("links"));
        }
        let base = parse_revision(base_revision)?;
        let (uid, channels) = self.verified_identity(device_id)?;
        let prepared = self.ensure_pending(&uid, channels)?;
        let status = self.status_for(base, &uid, channels, &prepared)?;
        if status == BindingStatus::Replacement && !replace_binding {
            return Err(consent_required());
        }

        let mut candidate = prepared;
        let parsed = parse_pairs(pairs)?;
        let (channel_map, source_defaults) =
            apply_source_links(&candidate.channel_map, &candidate.source_defaults, &parsed)
                .map_err(|_| invalid("the submitted links are not a complete, valid pair set"))?;
        candidate.channel_map = channel_map;
        candidate.source_defaults = source_defaults;

        self.commit(base, candidate, status)?;
        self.saved_view(&uid, channels)
    }

    /// Resolve the startup profile for the selected device.
    ///
    /// A saved compatible profile is returned unchanged. A missing profile is materialized once
    /// (centered) and a compatible resolved legacy profile is finalized, both through the prepared
    /// commit so preview/committed ids are preserved. An incompatible saved profile is rejected
    /// (never silently replaced), and an unresolved legacy profile is rejected until the operator
    /// resolves it via a save.
    pub fn resolve_start_profile(&mut self, device_id: &str) -> Result<ConfigV2, IpcError> {
        self.require_store()?;
        let (uid, channels) = self.verified_identity(device_id)?;
        let plan = {
            let store = self.require_store()?;
            plan_start(store, &uid, channels)?
        };
        match plan {
            StartPlan::Saved(config) => Ok(config),
            StartPlan::InstallNew(candidate) => {
                let base = self.require_store()?.revision();
                self.commit(base, candidate, BindingStatus::New)?;
                Ok(self.require_store()?.committed().clone())
            }
            StartPlan::FinalizeLegacy(candidate) => {
                let base = self.require_store()?.revision();
                self.commit(base, candidate, BindingStatus::Legacy)?;
                Ok(self.require_store()?.committed().clone())
            }
            StartPlan::Reject(message) => Err(ipc("SOURCE_SETTINGS_REPLACEMENT_REQUIRED", message)),
            StartPlan::Unresolved(message) => Err(ipc("SOURCE_SETTINGS_LEGACY_UNRESOLVED", message)),
        }
    }

    /// Verify the requested base revision and classify the selected device.
    fn status_for(
        &self,
        base: u64,
        uid: &str,
        channels: u16,
        prepared: &ConfigV2,
    ) -> Result<BindingStatus, IpcError> {
        let store = self.require_store()?;
        let found = store.revision();
        if base != found {
            return Err(revision_conflict(base, found));
        }
        Ok(binding_status(store, uid, channels, prepared))
    }

    fn view(&self, uid: &str, channels: u16, config: &ConfigV2) -> Result<SourceSettings, IpcError> {
        let revision = config.settings_revision.get();
        let status = binding_status(self.require_store()?, uid, channels, config);
        Ok(SourceSettings {
            device_id: uid.to_string(),
            input_channels: channels,
            settings_revision: revision.to_string(),
            binding_status: status,
            durability_uncertain: self.durability_uncertain,
            catalog: crate::bridge::catalog_from_core(&catalog_from_config(
                config,
                CatalogRevision(revision),
            )),
        })
    }

    /// The canonical saved view after a successful commit.
    fn saved_view(&mut self, uid: &str, channels: u16) -> Result<SourceSettings, IpcError> {
        let committed = self.require_store()?.committed().clone();
        self.view(uid, channels, &committed)
    }

    /// Commit `candidate` through the ordinary (saved binding) or prepared (new/legacy/replacement)
    /// path, retaining the uncertainty outcome truthfully.
    fn commit(&mut self, base: u64, candidate: ConfigV2, status: BindingStatus) -> Result<(), IpcError> {
        let outcome = {
            let store = match self.store.as_mut() {
                Some(store) => store,
                None => {
                    return Err(unreadable_error(
                        self.load_error.as_deref().unwrap_or("source settings are unreadable"),
                    ))
                }
            };
            if status == BindingStatus::Saved {
                store.commit(base, candidate)
            } else {
                store.commit_prepared_binding(base, candidate)
            }
        };
        match outcome {
            Ok(installed) => {
                self.durability_uncertain = false;
                self.pending = None;
                self.refresh_projection(&installed);
                Ok(())
            }
            Err(ConfigError::DurabilityUncertain { .. }) => {
                // The candidate is installed but durability is unconfirmed: retain the uncertainty
                // and project the installed state so a reload tells the truth.
                self.durability_uncertain = true;
                self.pending = None;
                if let Some(store) = self.store.as_ref() {
                    let installed = store.committed().clone();
                    self.refresh_projection(&installed);
                }
                Err(ipc(
                    "SOURCE_SETTINGS_DURABILITY_UNCERTAIN",
                    "settings were installed but restart durability is unconfirmed; reload before retrying",
                ))
            }
            Err(error) => Err(map_config_error(error)),
        }
    }

    fn refresh_projection(&self, config: &ConfigV2) {
        if let Ok(mut guard) = self.projection.write() {
            *guard = catalog_from_config(config, CatalogRevision(config.settings_revision.get()));
        }
    }

    fn verified_identity(&self, device_id: &str) -> Result<(String, u16), IpcError> {
        self.identity
            .verified(device_id, SAMPLE_RATE_HZ)
            .map_err(|message| {
                ipc(
                    "SOURCE_DEVICE_UNKNOWN",
                    format!("could not verify the selected device: {message}"),
                )
            })
    }
}

/// The decided startup binding action for the selected device.
pub enum StartPlan {
    /// A saved compatible profile: use it unchanged.
    Saved(ConfigV2),
    /// A missing profile: materialize the prepared candidate once.
    InstallNew(ConfigV2),
    /// A compatible resolved legacy profile: finalize it once.
    FinalizeLegacy(ConfigV2),
    /// An incompatible saved profile: never silently replace.
    Reject(String),
    /// An unresolved legacy profile: rejected until the operator resolves it.
    Unresolved(String),
}

/// Decide how startup should bind the selected device against the committed profile.
pub fn plan_start(store: &ConfigStore, uid: &str, channels: u16) -> Result<StartPlan, IpcError> {
    let committed = store.committed();
    if committed.device_id.is_none() {
        let candidate = prepare(store, uid, channels)?;
        return Ok(StartPlan::InstallNew(candidate));
    }
    if committed.device_id.as_deref() != Some(uid) {
        return Ok(StartPlan::Reject(
            "the selected device differs from the saved profile; save a replacement explicitly"
                .to_string(),
        ));
    }
    if committed.input_channels.is_none() {
        // A migrated legacy profile: prepare (retaining ids) and finalize only if fully resolved.
        let candidate = prepare(store, uid, channels)?;
        if validate_config(&candidate).is_ok() {
            return Ok(StartPlan::FinalizeLegacy(candidate));
        }
        return Ok(StartPlan::Unresolved(
            "resolve or unlink the legacy stereo pair before starting".to_string(),
        ));
    }
    if committed.input_channels != Some(channels) {
        return Ok(StartPlan::Reject(
            "the selected capture layout differs from the saved profile; save a replacement explicitly"
                .to_string(),
        ));
    }
    if validate_config(committed).is_ok() {
        Ok(StartPlan::Saved(committed.clone()))
    } else if store.has_unresolved_pairs() {
        Ok(StartPlan::Unresolved(
            "resolve or unlink the legacy stereo pair before starting".to_string(),
        ))
    } else {
        Ok(StartPlan::Reject(
            "the saved profile is incompatible with the selected device".to_string(),
        ))
    }
}

fn prepare(store: &ConfigStore, uid: &str, channels: u16) -> Result<ConfigV2, IpcError> {
    store
        .prepare_binding(uid, channels)
        .map_err(|error| ipc("SOURCE_SETTINGS_PREPARE_FAILED", error.to_string()))
}

/// Verify the actually-started capture layout matches the verified binding before advertising
/// server success. A mismatch is torn down and never silently rebound.
pub fn verify_started_channels(expected: u16, actual: u16) -> Result<(), IpcError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ipc(
            "CAPTURE_LAYOUT_MISMATCH",
            format!("started capture has {actual} input channels but the binding expects {expected}"),
        ))
    }
}

/// Classify the selected device against the committed profile for a view.
pub fn binding_status(
    store: &ConfigStore,
    uid: &str,
    channels: u16,
    prepared: &ConfigV2,
) -> BindingStatus {
    let committed = store.committed();
    if committed.device_id.is_none() {
        return BindingStatus::New;
    }
    if committed.device_id.as_deref() != Some(uid) {
        return BindingStatus::Replacement;
    }
    if committed.input_channels.is_none() {
        // A migrated legacy profile with no verified layout.
        return BindingStatus::Legacy;
    }
    if committed.input_channels != Some(channels) {
        return BindingStatus::Replacement;
    }
    if validate_config(prepared).is_ok() {
        BindingStatus::Saved
    } else {
        // Same device/layout but not yet resolvable (unresolved legacy links).
        BindingStatus::Legacy
    }
}

/// The initial committed projection for a loaded store (empty when unreadable/unbound).
pub fn projection_for(store: Option<&ConfigStore>) -> CatalogSnapshot {
    match store {
        Some(store) => catalog_from_config(store.committed(), CatalogRevision(store.revision())),
        None => empty_catalog(),
    }
}

/// An explicitly empty catalog (never a fabricated row set).
pub fn empty_catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog_revision: CatalogRevision(0),
        sources: Vec::new(),
    }
}

fn parse_defaults(defaults: &[SourcePanDefaultDto]) -> Result<Vec<SourcePanDefault>, IpcError> {
    defaults
        .iter()
        .map(|dto| {
            let source_id = parse_source_id(&dto.source_id)
                .ok_or_else(|| invalid("a source id is not a valid UUID"))?;
            Ok(SourcePanDefault {
                source_id,
                pan: dto.pan,
            })
        })
        .collect()
}

fn parse_pairs(pairs: &[SourceLinkPair]) -> Result<Vec<(SourceId, SourceId)>, IpcError> {
    pairs
        .iter()
        .map(|pair| {
            let left = parse_source_id(&pair.left_source_id)
                .ok_or_else(|| invalid("a source id is not a valid UUID"))?;
            let right = parse_source_id(&pair.right_source_id)
                .ok_or_else(|| invalid("a source id is not a valid UUID"))?;
            Ok((left, right))
        })
        .collect()
}

/// Parse an opaque source-id UUID string, rejecting anything that is not a UUID.
pub fn parse_source_id(raw: &str) -> Option<SourceId> {
    uuid::Uuid::parse_str(raw).ok().map(SourceId::from)
}

fn parse_revision(raw: &str) -> Result<u64, IpcError> {
    raw.parse::<u64>().map_err(|_| {
        ipc(
            "SOURCE_SETTINGS_REVISION_INVALID",
            "base revision must be a decimal counter string",
        )
    })
}

fn revision_conflict(expected: u64, found: u64) -> IpcError {
    ipc(
        "SOURCE_SETTINGS_REVISION_CONFLICT",
        format!("base revision {expected} does not match the committed revision {found}"),
    )
}

fn consent_required() -> IpcError {
    ipc(
        "SOURCE_SETTINGS_REPLACEMENT_CONSENT_REQUIRED",
        "replacing the saved device binding requires explicit consent",
    )
}

fn host_running_error(action: &str) -> IpcError {
    ipc(
        "SOURCE_SETTINGS_HOST_RUNNING",
        format!("saving {action} for a new or replacement binding requires a stopped host"),
    )
}

fn invalid(message: &str) -> IpcError {
    ipc("SOURCE_SETTINGS_INVALID", message)
}

fn unreadable_error(message: &str) -> IpcError {
    ipc("SOURCE_SETTINGS_UNREADABLE", message)
}

fn map_config_error(error: ConfigError) -> IpcError {
    match error {
        ConfigError::RevisionConflict { expected, found } => revision_conflict(expected, found),
        ConfigError::DeviceMismatch { .. } => consent_required(),
        ConfigError::RevisionExhausted => ipc(
            "SOURCE_SETTINGS_REVISION_EXHAUSTED",
            "the settings revision is exhausted; no further save is possible",
        ),
        // Retained for totality: the commit path intercepts this variant first so it can reconcile
        // memory/projection before returning. Mapping it here keeps the outcome truthful regardless.
        ConfigError::DurabilityUncertain { .. } => ipc(
            "SOURCE_SETTINGS_DURABILITY_UNCERTAIN",
            "settings were installed but restart durability is unconfirmed; reload before retrying",
        ),
        other => ipc("SOURCE_SETTINGS_SAVE_FAILED", other.to_string()),
    }
}

fn ipc(code: &str, message: impl Into<String>) -> IpcError {
    IpcError {
        code: code.to_string(),
        message: message.into(),
    }
}

/// Build the initial catalog snapshot for a read view from a config (reuse the bridge conversion).
pub fn bridge_catalog(config: &ConfigV2) -> BridgeCatalogSnapshot {
    crate::bridge::catalog_from_core(&catalog_from_config(
        config,
        CatalogRevision(config.settings_revision.get()),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::bridge::BindingStatus;

    /// A deterministic identity resolver: maps a device id (or UID) to `(uid, channels)`.
    ///
    /// Tests never touch hardware or real user data. An unknown device is an explicit error, never
    /// a silent fallback to two channels.
    struct FakeIdentity {
        by_device: HashMap<String, (String, u16)>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeIdentity {
        fn new(pairs: &[(&str, &str, u16)]) -> Arc<Self> {
            Arc::new(Self {
                by_device: pairs
                    .iter()
                    .map(|(device, uid, channels)| (device.to_string(), (uid.to_string(), *channels)))
                    .collect(),
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    impl CaptureIdentity for FakeIdentity {
        fn verified(&self, device_id: &str, _rate: u32) -> Result<(String, u16), String> {
            self.calls.lock().unwrap().push(device_id.to_string());
            self.by_device
                .get(device_id)
                .cloned()
                .ok_or_else(|| format!("unknown device {device_id}"))
        }
    }

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iem-cast-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn manager(tag: &str, identity: Arc<FakeIdentity>) -> (SettingsManager, PathBuf) {
        let dir = unique_temp_dir(tag);
        let path = dir.join("source-settings.json");
        (SettingsManager::open(path.clone(), identity), path)
    }

    fn defaults_for(config: &ConfigV2, pan: f32) -> Vec<SourcePanDefaultDto> {
        config
            .channel_map
            .iter()
            .map(|entry| SourcePanDefaultDto {
                source_id: entry.source_id.to_string(),
                pan,
            })
            .collect()
    }

    #[test]
    fn repeated_read_preview_is_stable_and_never_writes() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, path) = manager("preview-stable", Arc::clone(&identity));

        let first = manager.read_view("dev").expect("first read");
        let second = manager.read_view("dev").expect("second read");

        // The preview UUIDs are identical across reads (no churn).
        let first_ids: Vec<&str> = first
            .catalog
            .sources
            .iter()
            .map(|s| s.source_id.as_str())
            .collect();
        let second_ids: Vec<&str> = second
            .catalog
            .sources
            .iter()
            .map(|s| s.source_id.as_str())
            .collect();
        assert_eq!(first_ids, second_ids, "preview ids are stable across reads");
        assert_eq!(first.binding_status, BindingStatus::New);
        assert_eq!(first.settings_revision, "0");

        // No file is created by a read.
        assert!(!path.exists(), "a read never writes the settings file");
        assert_eq!(identity.calls.lock().unwrap().len(), 2, "identity resolved per read");
    }

    #[test]
    fn read_never_overwrites_a_saved_file() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, path) = manager("read-no-overwrite", identity);
        // Save an initial binding + defaults.
        manager.read_view("dev").unwrap();
        let candidate = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev", "0", &defaults_for(&candidate, 0.25), false)
            .unwrap();
        let bytes_before = fs::read(&path).unwrap();

        // A read must not change the committed file.
        manager.read_view("dev").unwrap();
        manager.read_view("dev").unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes_before, "read never rewrites saved data");
    }

    #[test]
    fn pending_preview_ids_are_retained_through_save_and_start() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, _path) = manager("preview-persist", identity);

        let preview = manager.read_view("dev").expect("preview");
        let preview_ids: Vec<String> = preview
            .catalog
            .sources
            .iter()
            .map(|s| s.source_id.clone())
            .collect();
        let pending = manager.pending.as_ref().unwrap().candidate.clone();

        let saved = manager
            .save_defaults(false, "dev", "0", &defaults_for(&pending, -0.5), false)
            .expect("save prepared binding");
        let saved_ids: Vec<String> = saved.catalog.sources.iter().map(|s| s.source_id.clone()).collect();
        assert_eq!(saved_ids, preview_ids, "save installs the exact preview ids");
        assert_eq!(saved.binding_status, BindingStatus::Saved);
        assert_eq!(saved.settings_revision, "1");

        // Start resolves the same committed identity.
        let profile = manager.resolve_start_profile("dev").expect("start profile");
        let start_ids: Vec<String> = profile
            .channel_map
            .iter()
            .map(|e| e.source_id.to_string())
            .collect();
        assert_eq!(start_ids, preview_ids, "start uses the retained map");
    }

    #[test]
    fn replacement_requires_consent_and_a_stopped_host() {
        let identity = FakeIdentity::new(&[("dev-a", "uid-A", 2), ("dev-b", "uid-B", 2)]);
        let (mut manager, _path) = manager("replacement-consent", identity);
        // Bind device A.
        manager.read_view("dev-a").unwrap();
        let candidate = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev-a", "0", &defaults_for(&candidate, 0.0), false)
            .unwrap();

        // A different device is a replacement: no consent -> rejected with no write.
        let view_b = manager.read_view("dev-b").expect("replacement preview");
        assert_eq!(view_b.binding_status, BindingStatus::Replacement);
        let err = manager
            .save_defaults(false, "dev-b", "1", &defaults_for(&view_b_candidate(&manager), 0.0), false)
            .expect_err("consent required");
        assert_eq!(err.code, "SOURCE_SETTINGS_REPLACEMENT_CONSENT_REQUIRED");
        assert_eq!(manager.require_store().unwrap().revision(), 1, "no write on refusal");

        // Consent but a running host -> still refused (stopped host required).
        let err = manager
            .save_defaults(true, "dev-b", "1", &defaults_for(&view_b_candidate(&manager), 0.0), true)
            .expect_err("stopped host required");
        assert_eq!(err.code, "SOURCE_SETTINGS_HOST_RUNNING");

        // Consent + stopped host -> accepted.
        let saved = manager
            .save_defaults(false, "dev-b", "1", &defaults_for(&view_b_candidate(&manager), 0.0), true)
            .expect("replacement accepted");
        assert_eq!(saved.binding_status, BindingStatus::Saved);
    }

    fn view_b_candidate(manager: &SettingsManager) -> ConfigV2 {
        manager.pending.as_ref().unwrap().candidate.clone()
    }

    #[test]
    fn stale_base_revision_is_rejected_without_writing() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, path) = manager("stale-revision", identity);
        manager.read_view("dev").unwrap();
        let pending = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev", "0", &defaults_for(&pending, 0.0), false)
            .unwrap();
        let bytes_before = fs::read(&path).unwrap();

        // Base "0" is now stale (committed is 1).
        let err = manager
            .save_defaults(false, "dev", "0", &defaults_for(&pending, 0.5), false)
            .expect_err("stale revision");
        assert_eq!(err.code, "SOURCE_SETTINGS_REVISION_CONFLICT");
        assert_eq!(fs::read(&path).unwrap(), bytes_before, "no write on stale save");
    }

    #[test]
    fn wrong_device_save_does_not_touch_the_active_binding() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, _path) = manager("wrong-device-save", identity);
        manager.read_view("dev").unwrap();
        let candidate = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev", "0", &defaults_for(&candidate, 0.0), false)
            .unwrap();
        let committed_before = manager.require_store().unwrap().committed().clone();

        // A save for an unknown device cannot prepare and must not mutate the binding.
        let err = manager
            .save_defaults(false, "unknown-dev", "1", &[], false)
            .expect_err("unknown device");
        assert_eq!(err.code, "SOURCE_DEVICE_UNKNOWN");
        assert_eq!(
            manager.require_store().unwrap().committed(),
            &committed_before,
            "active binding unchanged"
        );
    }

    #[test]
    fn corrupt_load_surfaces_a_visible_error_and_never_overwrites() {
        let dir = unique_temp_dir("corrupt-load");
        let path = dir.join("source-settings.json");
        fs::write(&path, b"{ not valid json").unwrap();
        let bytes_before = fs::read(&path).unwrap();
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let mut manager = SettingsManager::open(path.clone(), identity);

        assert!(manager.unreadable().is_some(), "corrupt load is a visible error");
        let err = manager.read_view("dev").expect_err("read refuses a corrupt store");
        assert_eq!(err.code, "SOURCE_SETTINGS_UNREADABLE");
        assert_eq!(fs::read(&path).unwrap(), bytes_before, "corrupt bytes are never overwritten");
    }

    #[test]
    fn start_mismatch_binding_is_rejected_without_writing() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2), ("dev-4ch", "uid-A", 4)]);
        let (mut manager, _path) = manager("start-mismatch", identity);
        manager.read_view("dev").unwrap();
        let candidate = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev", "0", &defaults_for(&candidate, 0.0), false)
            .unwrap();
        let committed_before = manager.require_store().unwrap().committed().clone();

        // Same UID but a different verified layout: never silently replaced.
        let err = manager
            .resolve_start_profile("dev-4ch")
            .expect_err("incompatible layout");
        assert_eq!(err.code, "SOURCE_SETTINGS_REPLACEMENT_REQUIRED");
        assert_eq!(
            manager.require_store().unwrap().committed(),
            &committed_before,
            "start never silently rebinds"
        );
    }

    #[test]
    fn start_does_not_resolve_legacy_pair_sides() {
        // A migrated legacy profile with an unresolved pair must not be auto-resolved at start.
        let dir = unique_temp_dir("legacy-unresolved");
        let path = dir.join("source-settings.json");
        // A v1 file: a stereo peer without an explicit side.
        let source_a = uuid::Uuid::new_v4();
        let source_b = uuid::Uuid::new_v4();
        let legacy = serde_json::json!({
            "version": 1,
            "deviceId": "uid-A",
            "selectedInterface": null,
            "channelMap": [
                { "physicalIndex": 0, "sourceId": source_a, "stereoPair": source_b, "role": "inputChannel" },
                { "physicalIndex": 1, "sourceId": source_b, "stereoPair": source_a, "role": "inputChannel" }
            ],
            "labels": []
        });
        fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let mut manager = SettingsManager::open(path, identity);

        let err = manager
            .resolve_start_profile("dev")
            .expect_err("unresolved legacy must not start");
        assert_eq!(err.code, "SOURCE_SETTINGS_LEGACY_UNRESOLVED");
    }

    #[test]
    fn save_links_requires_a_stopped_host() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, _path) = manager("links-stopped", identity);
        manager.read_view("dev").unwrap();

        let err = manager
            .save_links(true, "dev", "0", &[], false)
            .expect_err("links need a stopped host");
        assert_eq!(err.code, "SOURCE_SETTINGS_HOST_RUNNING");
    }

    #[test]
    fn save_links_recenters_affected_defaults_and_saves_atomically() {
        let identity = FakeIdentity::new(&[("dev", "uid-A", 4)]);
        let (mut manager, path) = manager("links-save", identity);
        manager.read_view("dev").unwrap();
        let map = manager.pending.as_ref().unwrap().candidate.channel_map.clone();
        // Defaults with non-center values on the two sources to be linked.
        let defaults: Vec<SourcePanDefaultDto> = map
            .iter()
            .enumerate()
            .map(|(index, entry)| SourcePanDefaultDto {
                source_id: entry.source_id.to_string(),
                pan: if index < 2 { -0.5 } else { 0.0 },
            })
            .collect();
        manager.save_defaults(false, "dev", "0", &defaults, false).unwrap();

        let pair = SourceLinkPair {
            left_source_id: map[0].source_id.to_string(),
            right_source_id: map[1].source_id.to_string(),
        };
        let saved = manager
            .save_links(false, "dev", "1", std::slice::from_ref(&pair), false)
            .expect("save links");
        // The two linked members are centered; the unrelated sources are unchanged.
        let pan_for = |id: &str| {
            saved
                .catalog
                .sources
                .iter()
                .find(|s| s.source_id == id)
                .map(|s| s.default_pan)
                .unwrap()
        };
        assert_eq!(pan_for(&map[0].source_id.to_string()), 0.0, "affected default centered");
        assert_eq!(pan_for(&map[1].source_id.to_string()), 0.0);
        assert!(saved.catalog.sources[0].stereo_pair.is_some(), "link is present in the projection");
        assert!(path.exists(), "links persist to the file");
    }

    #[test]
    fn verify_started_channels_tears_down_on_mismatch() {
        assert!(verify_started_channels(2, 2).is_ok());
        let err = verify_started_channels(2, 4).expect_err("mismatch");
        assert_eq!(err.code, "CAPTURE_LAYOUT_MISMATCH");
    }

    #[test]
    fn uncertain_outcome_and_revision_errors_map_to_truthful_typed_codes() {
        // The desktop cannot inject a host-core directory-sync fault (that seam is core-private),
        // so the mapping of each store outcome to a clear, distinct IPC code is unit-tested here.
        let uncertain = map_config_error(ConfigError::DurabilityUncertain {
            revision: 3,
            source: std::io::Error::other("sync dir failed"),
        });
        assert_eq!(uncertain.code, "SOURCE_SETTINGS_DURABILITY_UNCERTAIN");

        let stale = map_config_error(ConfigError::RevisionConflict {
            expected: 1,
            found: 2,
        });
        assert_eq!(stale.code, "SOURCE_SETTINGS_REVISION_CONFLICT");

        let exhausted = map_config_error(ConfigError::RevisionExhausted);
        assert_eq!(exhausted.code, "SOURCE_SETTINGS_REVISION_EXHAUSTED");
    }

    #[test]
    fn projection_cache_tracks_installed_defaults_without_a_live_catalog_bump() {
        // A defaults-only save updates the projection cache a fresh connection reads, while the
        // committed catalog revision advances only with the settings revision (no routing bump).
        let identity = FakeIdentity::new(&[("dev", "uid-A", 2)]);
        let (mut manager, _path) = manager("projection-cache", identity);
        let projection = manager.projection();
        let before = projection.read().unwrap().clone();
        assert_eq!(before.catalog_revision, CatalogRevision(0));

        manager.read_view("dev").unwrap();
        let candidate = manager.pending.as_ref().unwrap().candidate.clone();
        manager
            .save_defaults(false, "dev", "0", &defaults_for(&candidate, 0.75), false)
            .unwrap();

        let after = projection.read().unwrap().clone();
        assert_eq!(after.catalog_revision, CatalogRevision(1));
        assert!(after.sources.iter().any(|s| s.default_pan == 0.75), "fresh connection sees B");
    }
}
