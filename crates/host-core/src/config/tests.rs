//! Transactional config store tests.
//!
//! Every test uses its own unique temp directory and removes it on the way out; no shared path and
//! no broad `rm`. The store path lives under that directory. No test opens real hardware. The
//! durability tests drive the private deterministic fault seam, which is `#[cfg(test)]`-only.

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use super::*;
use crate::contract::{
    ChannelMapEntry, Config, ConfigV2, SourcePanDefault, SourceRole, StereoSide,
};
use crate::ids::{SettingsRevision, SourceId};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("iem-cast-cfg-{tag}-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn sid(byte: u8) -> SourceId {
    SourceId::from_bytes([byte; 16])
}

fn entry(index: u16, byte: u8) -> ChannelMapEntry {
    ChannelMapEntry {
        physical_index: index,
        source_id: sid(byte),
        stereo_pair: None,
        stereo_side: None,
        role: SourceRole::InputChannel,
    }
}

fn candidate(
    device: Option<&str>,
    channels: Option<u16>,
    channel_map: Vec<ChannelMapEntry>,
    source_defaults: Vec<SourcePanDefault>,
) -> ConfigV2 {
    ConfigV2 {
        version: CONFIG_VERSION,
        device_id: device.map(str::to_string),
        selected_interface: None,
        input_channels: channels,
        settings_revision: SettingsRevision(0),
        channel_map,
        labels: Vec::new(),
        source_defaults,
    }
}

fn centered(map: &[ChannelMapEntry]) -> Vec<SourcePanDefault> {
    centered_defaults(map)
}

/// Set the deterministic fault stage for the next save.
fn inject(store: &mut ConfigStore, stage: WriteStage) {
    store.fault = Some(stage);
}

// ---------------------------------------------------------------------------------------------
// open / fresh / corrupt / unsupported
// ---------------------------------------------------------------------------------------------

#[test]
fn missing_file_opens_fresh_unbound_empty_config() {
    let dir = temp_dir("fresh");
    let path = dir.join("config.json");
    assert!(!path.exists());

    let store = ConfigStore::open(&path).expect("a missing file is a fresh store");

    assert_eq!(store.binding(), None);
    assert_eq!(store.revision(), 0);
    assert_eq!(store.committed().version, CONFIG_VERSION);
    assert!(store.committed().channel_map.is_empty());
    assert!(store.committed().source_defaults.is_empty());
    assert_eq!(store.committed().settings_revision, SettingsRevision(0));
    assert!(!path.exists(), "open must not create the config file");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn corrupt_config_is_surfaced_file_bytes_unchanged() {
    let dir = temp_dir("corrupt");
    let path = dir.join("config.json");
    let garbage = b"{ this is not json".to_vec();
    fs::write(&path, &garbage).expect("write garbage");

    let result = ConfigStore::open(&path);
    assert!(
        matches!(result, Err(ConfigError::Corrupt)),
        "garbage must surface as Corrupt, got {result:?}"
    );
    assert_eq!(
        fs::read(&path).expect("read back"),
        garbage,
        "a corrupt file must never be overwritten"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unsupported_version_is_surfaced_and_never_overwritten() {
    let dir = temp_dir("unsupported");
    let path = dir.join("config.json");
    let bytes = br#"{"version":3}"#.to_vec();
    fs::write(&path, &bytes).expect("write future version");

    let result = ConfigStore::open(&path);
    assert!(
        matches!(result, Err(ConfigError::Unsupported(3))),
        "an unknown version must surface as Unsupported, got {result:?}"
    );
    assert_eq!(fs::read(&path).expect("read back"), bytes);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn invalid_v2_metadata_is_corrupt_and_bytes_unchanged() {
    // A current-version file with a non-reciprocal pair, an out-of-bounds index, or unequal pair
    // pans is corrupt. The original bytes must be left untouched.
    let cases = [
        // Non-reciprocal pair (peer points back but sides are equal).
        r#"{"version":2,"deviceId":"d","selectedInterface":null,"inputChannels":2,"settingsRevision":"1","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-4111-8111-111111111111","stereoPair":"22222222-2222-4222-8222-222222222222","stereoSide":"left","role":"inputChannel"},{"physicalIndex":1,"sourceId":"22222222-2222-4222-8222-222222222222","stereoPair":"11111111-1111-4111-8111-111111111111","stereoSide":"left","role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-4111-8111-111111111111","pan":0.0},{"sourceId":"22222222-2222-4222-8222-222222222222","pan":0.0}]}"#,
        // Physical index 99 for a 2-channel layout.
        r#"{"version":2,"deviceId":"d","selectedInterface":null,"inputChannels":2,"settingsRevision":"1","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-4111-8111-111111111111","stereoPair":null,"stereoSide":null,"role":"inputChannel"},{"physicalIndex":99,"sourceId":"22222222-2222-4222-8222-222222222222","stereoPair":null,"stereoSide":null,"role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-4111-8111-111111111111","pan":0.0},{"sourceId":"22222222-2222-4222-8222-222222222222","pan":0.0}]}"#,
        // A resolved pair with unequal defaults.
        r#"{"version":2,"deviceId":"d","selectedInterface":null,"inputChannels":2,"settingsRevision":"1","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-4111-8111-111111111111","stereoPair":"22222222-2222-4222-8222-222222222222","stereoSide":"left","role":"inputChannel"},{"physicalIndex":1,"sourceId":"22222222-2222-4222-8222-222222222222","stereoPair":"11111111-1111-4111-8111-111111111111","stereoSide":"right","role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-4111-8111-111111111111","pan":0.5},{"sourceId":"22222222-2222-4222-8222-222222222222","pan":-0.5}]}"#,
    ];

    for (index, body) in cases.iter().enumerate() {
        let dir = temp_dir(&format!("invalid-v2-{index}"));
        let path = dir.join("config.json");
        fs::write(&path, body.as_bytes()).expect("write invalid v2");

        let result = ConfigStore::open(&path);
        assert!(
            matches!(result, Err(ConfigError::Corrupt)),
            "case {index} must be corrupt, got {result:?}"
        );
        assert_eq!(
            fs::read(&path).expect("read back"),
            body.as_bytes(),
            "case {index} bytes must be unchanged"
        );

        fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn sparse_v2_defaults_load_centered_then_full_validation_applies() {
    // A valid map with only one default recorded: the missing entry normalizes to center on load,
    // and the loaded vector is complete.
    let dir = temp_dir("sparse-v2");
    let path = dir.join("config.json");
    let body = r#"{"version":2,"deviceId":"d","selectedInterface":null,"inputChannels":2,"settingsRevision":"5","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-1111-1111-111111111111","stereoPair":null,"stereoSide":null,"role":"inputChannel"},{"physicalIndex":1,"sourceId":"22222222-2222-2222-2222-222222222222","stereoPair":null,"stereoSide":null,"role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-1111-1111-111111111111","pan":0.25}]}"#;
    fs::write(&path, body.as_bytes()).expect("write sparse v2");

    let store = ConfigStore::open(&path).expect("sparse defaults normalize");
    assert_eq!(store.revision(), 5);
    assert_eq!(store.committed().source_defaults.len(), 2);
    assert_eq!(
        store
            .committed()
            .source_defaults
            .iter()
            .find(|d| d.source_id == sid(0x11))
            .unwrap()
            .pan,
        0.25
    );
    assert_eq!(
        store
            .committed()
            .source_defaults
            .iter()
            .find(|d| d.source_id == sid(0x22))
            .unwrap()
            .pan,
        0.0
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn populated_v2_null_layout_is_corrupt_and_bytes_unchanged() {
    // A device-bound map with an out-of-bounds index but `inputChannels: null` no longer bypasses
    // count/index validation: it is corrupt and its bytes are left untouched.
    let dir = temp_dir("v2-null-layout");
    let path = dir.join("config.json");
    let body = r#"{"version":2,"deviceId":"deviceA","selectedInterface":null,"inputChannels":null,"settingsRevision":"1","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-4111-8111-111111111111","stereoPair":null,"stereoSide":null,"role":"inputChannel"},{"physicalIndex":99,"sourceId":"22222222-2222-4222-8222-222222222222","stereoPair":null,"stereoSide":null,"role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-4111-8111-111111111111","pan":0.0},{"sourceId":"22222222-2222-4222-8222-222222222222","pan":0.0}]}"#;
    fs::write(&path, body.as_bytes()).expect("write populated null-layout v2");

    let result = ConfigStore::open(&path);
    assert!(
        matches!(result, Err(ConfigError::Corrupt)),
        "a populated v2 map without a count must be corrupt, got {result:?}"
    );
    assert_eq!(
        fs::read(&path).expect("read back"),
        body.as_bytes(),
        "bytes must be unchanged"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unbound_populated_v2_is_corrupt() {
    // No device, no count, but a populated map: an inconsistent unbound binding.
    let dir = temp_dir("v2-unbound-populated");
    let path = dir.join("config.json");
    let body = r#"{"version":2,"deviceId":null,"selectedInterface":null,"inputChannels":null,"settingsRevision":"1","channelMap":[{"physicalIndex":0,"sourceId":"11111111-1111-4111-8111-111111111111","stereoPair":null,"stereoSide":null,"role":"inputChannel"}],"labels":[],"sourceDefaults":[{"sourceId":"11111111-1111-4111-8111-111111111111","pan":0.0}]}"#;
    fs::write(&path, body.as_bytes()).expect("write unbound populated v2");

    let result = ConfigStore::open(&path);
    assert!(
        matches!(result, Err(ConfigError::Corrupt)),
        "an unbound populated v2 must be corrupt, got {result:?}"
    );
    assert_eq!(fs::read(&path).expect("read back"), body.as_bytes());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_unbound_v2_loads_valid() {
    // Only the empty unbound base state may omit the layout.
    let dir = temp_dir("v2-empty");
    let path = dir.join("config.json");
    let body = r#"{"version":2,"deviceId":null,"selectedInterface":null,"inputChannels":null,"settingsRevision":"0","channelMap":[],"labels":[],"sourceDefaults":[]}"#;
    fs::write(&path, body.as_bytes()).expect("write empty unbound v2");

    let store = ConfigStore::open(&path).expect("an empty unbound v2 is valid");
    assert_eq!(store.revision(), 0);
    assert_eq!(store.binding(), None);
    assert!(store.committed().channel_map.is_empty());
    assert!(store.committed().source_defaults.is_empty());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unbound_candidate_with_a_count_but_no_device_is_rejected() {
    // A count without a bound device is an inconsistent binding; nothing is written.
    let dir = temp_dir("none-device-count-some");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    let cand = candidate(
        None,
        Some(1),
        map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: 0.0 }],
    );
    assert!(
        matches!(store.commit(0, cand), Err(ConfigError::Corrupt)),
        "a count without a bound device must be rejected"
    );
    assert_eq!(store.revision(), 0);
    assert!(!path.exists(), "no file written on rejection");

    fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// migration
// ---------------------------------------------------------------------------------------------

#[test]
fn v1_migrates_to_v2_preserving_ids_labels_and_centered_defaults() {
    let dir = temp_dir("v1");
    let path = dir.join("config.json");
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: Some("en0".to_string()),
        channel_map: vec![entry(0, 0x11), entry(1, 0x22)],
        labels: vec![(sid(0x11), "Kick".to_string())],
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");

    let store = ConfigStore::open(&path).expect("v1 migrates");
    let committed = store.committed();

    assert_eq!(committed.version, CONFIG_VERSION);
    assert_eq!(committed.device_id.as_deref(), Some("dev-A"));
    assert_eq!(committed.selected_interface.as_deref(), Some("en0"));
    assert_eq!(committed.channel_map, v1.channel_map, "mapping preserved");
    assert_eq!(committed.labels, v1.labels, "labels preserved");
    assert_eq!(committed.source_defaults.len(), 2);
    assert!(
        committed.source_defaults.iter().all(|d| d.pan == 0.0),
        "migrated defaults are center"
    );
    assert_eq!(committed.settings_revision, SettingsRevision(0));
    assert_eq!(store.revision(), 0);
    assert_eq!(store.binding(), Some("dev-A"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn v1_migration_then_layout_verification_retains_ids_then_side_resolution_persists() {
    // v1 with a legacy unresolved pair. Same UID verification must retain the existing ids/map and
    // preserve the unresolved link in memory; after the operator assigns sides, the resolved config
    // saves and reopens as valid v2.
    let dir = temp_dir("v1-verify-resolve");
    let path = dir.join("config.json");
    let mut map = vec![entry(0, 0x11), entry(1, 0x22)];
    map[0].stereo_pair = Some(sid(0x22));
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: None,
        channel_map: map,
        labels: vec![(sid(0x11), "Kick".to_string())],
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");
    let original_bytes = fs::read(&path).unwrap();

    let mut store = ConfigStore::open(&path).expect("legacy load");
    let ids_before: Vec<SourceId> = store.committed().channel_map.iter().map(|e| e.source_id).collect();

    // Same UID, unknown legacy layout (v1 has no input_channels): verify, do not regenerate.
    let verified = store
        .verify_layout("dev-A", 2, 0)
        .expect("same-device verification retains the map");
    let ids_after: Vec<SourceId> = verified.channel_map.iter().map(|e| e.source_id).collect();
    assert_eq!(ids_before, ids_after, "verification must retain source ids");
    assert_eq!(verified.labels.len(), 1, "labels retained");
    assert!(store.has_unresolved_pairs(), "the unresolved link is still visible for the UI");
    // Unresolved: adopted in memory only, never written as invalid v2, revision unchanged.
    assert_eq!(store.revision(), 0);
    assert_eq!(fs::read(&path).unwrap(), original_bytes, "no invalid v2 file written");

    // The operator explicitly resolves the sides via the link editor; the resolve path persists.
    let (resolved_map, resolved_defaults) = apply_source_links(
        &store.committed().channel_map,
        &store.committed().source_defaults,
        &[(sid(0x11), sid(0x22))],
    )
    .expect("explicit link resolves the pair");
    let resolved = candidate(Some("dev-A"), Some(2), resolved_map, resolved_defaults);
    store.commit(0, resolved).expect("resolved config saves");

    let reopened = ConfigStore::open(&path).expect("resolved v2 reopens");
    assert_eq!(reopened.revision(), 1);
    assert_eq!(reopened.committed().channel_map[0].stereo_side, Some(StereoSide::Left));
    assert_eq!(reopened.committed().channel_map[1].stereo_side, Some(StereoSide::Right));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn v1_unresolved_pair_loads_for_ui_but_cannot_be_committed() {
    let dir = temp_dir("v1-unresolved");
    let path = dir.join("config.json");
    let mut map = vec![entry(0, 0x11), entry(1, 0x22)];
    map[0].stereo_pair = Some(sid(0x22));
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: None,
        channel_map: map,
        labels: Vec::new(),
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");

    let mut store = ConfigStore::open(&path).expect("legacy load is preserved for the UI");
    assert!(store.has_unresolved_pairs());

    let unresolved = store.committed().channel_map.clone();
    let defaults = store.committed().source_defaults.clone();
    let cand = candidate(Some("dev-A"), None, unresolved, defaults);
    assert!(
        matches!(store.commit(0, cand), Err(ConfigError::Corrupt)),
        "an unresolved pair cannot be persisted"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn v1_resolved_map_cannot_commit_directly_before_verification() {
    // A migrated v1 state is device-bound but has no verified count. Committing it directly (without
    // `verify_layout`) is rejected and leaves the original bytes/revision unchanged; verification
    // then produces a committed layout.
    let dir = temp_dir("v1-direct");
    let path = dir.join("config.json");
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: None,
        channel_map: vec![entry(0, 0x11), entry(1, 0x22)],
        labels: vec![(sid(0x11), "Kick".to_string())],
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");
    let original_bytes = fs::read(&path).unwrap();

    let mut store = ConfigStore::open(&path).expect("legacy load");
    let direct = candidate(
        Some("dev-A"),
        None,
        store.committed().channel_map.clone(),
        store.committed().source_defaults.clone(),
    );
    assert!(
        matches!(store.commit(0, direct), Err(ConfigError::Corrupt)),
        "a migrated unverified layout must not commit directly"
    );
    assert_eq!(store.revision(), 0);
    assert_eq!(fs::read(&path).unwrap(), original_bytes, "bytes unchanged");

    store.verify_layout("dev-A", 2, 0).expect("verified layout saves");
    assert_eq!(store.revision(), 1);

    fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// binding / commit
// ---------------------------------------------------------------------------------------------

#[test]
fn wrong_device_binding_is_rejected_and_committed_device_stays() {
    let dir = temp_dir("wrong-device");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 2, 0).expect("bind A");

    let cand = candidate(
        Some("B"),
        Some(2),
        store.committed().channel_map.clone(),
        centered(&store.committed().channel_map),
    );
    let error = store.commit(1, cand).expect_err("binding change must reject");
    match error {
        ConfigError::DeviceMismatch { bound, requested } => {
            assert_eq!(bound, "A");
            assert_eq!(requested, "B");
        }
        other => panic!("expected DeviceMismatch, got {other:?}"),
    }
    assert_eq!(store.committed().device_id.as_deref(), Some("A"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ordinary_commit_rejects_a_layout_change_under_the_same_uid() {
    // Verified binding has 2 channels. An ordinary commit that changes the count (or the
    // id/physical-index binding) is rejected, with the file and revision unchanged.
    let dir = temp_dir("commit-layout-change");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 2, 0).expect("bind A");
    let bytes_before = fs::read(&path).unwrap();
    let ids_before: Vec<SourceId> = store.committed().channel_map.iter().map(|e| e.source_id).collect();

    // Change the verified channel count.
    let changed_count = candidate(
        Some("A"),
        Some(3),
        vec![entry(0, 0x11), entry(1, 0x22), entry(2, 0x33)],
        vec![
            SourcePanDefault { source_id: sid(0x11), pan: 0.0 },
            SourcePanDefault { source_id: sid(0x22), pan: 0.0 },
            SourcePanDefault { source_id: sid(0x33), pan: 0.0 },
        ],
    );
    assert!(
        matches!(store.commit(1, changed_count), Err(ConfigError::Corrupt)),
        "a layout change must be an explicit bind"
    );

    // Change the id/physical-index binding at the same count.
    let mut rebound_map = store.committed().channel_map.clone();
    rebound_map[0].source_id = sid(0x99);
    let changed_ids = candidate(
        Some("A"),
        Some(2),
        rebound_map,
        vec![
            SourcePanDefault { source_id: sid(0x99), pan: 0.0 },
            SourcePanDefault { source_id: sid(0x22), pan: 0.0 },
        ],
    );
    assert!(
        matches!(store.commit(1, changed_ids), Err(ConfigError::Corrupt)),
        "a source-id rebinding must be an explicit bind"
    );

    assert_eq!(store.revision(), 1, "revision unchanged");
    assert_eq!(
        store.committed().channel_map.iter().map(|e| e.source_id).collect::<Vec<_>>(),
        ids_before
    );
    assert_eq!(fs::read(&path).unwrap(), bytes_before, "file unchanged");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn commit_with_stale_revision_is_rejected_and_memory_unchanged() {
    let dir = temp_dir("stale");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    let cand = candidate(Some("dev"), Some(1), map.clone(), centered(&map));
    store.commit(0, cand.clone()).expect("first commit");
    assert_eq!(store.revision(), 1);
    let before = store.committed().clone();

    let error = store.commit(0, cand).expect_err("stale revision must reject");
    assert!(matches!(
        error,
        ConfigError::RevisionConflict {
            expected: 0,
            found: 1
        }
    ));
    assert_eq!(store.committed(), &before, "memory must be unchanged");
    assert_eq!(store.revision(), 1);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_commit_retains_committed_revision_and_memory() {
    let root = temp_dir("io-fail");
    let conf = root.join("conf");
    fs::create_dir_all(&conf).unwrap();
    let path = conf.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    // Establish a valid bound baseline before breaking the directory.
    store.bind("dev", 1, 0).expect("bind baseline");

    // Replace the parent directory with a regular file so the temp-file write cannot succeed.
    fs::remove_dir_all(&conf).unwrap();
    fs::write(&conf, b"not a directory").unwrap();

    let map = store.committed().channel_map.clone();
    let cand = candidate(Some("dev"), Some(1), map.clone(), centered(&map));
    let error = store.commit(1, cand).expect_err("unwritable path must fail");
    assert!(matches!(error, ConfigError::Io(_)), "expected Io, got {error:?}");
    assert_eq!(store.revision(), 1, "revision must not advance on a failed save");
    assert_eq!(store.committed().device_id.as_deref(), Some("dev"));

    fs::remove_file(&conf).ok();
    fs::remove_dir_all(&root).ok();
}

#[test]
fn explicit_rebind_to_a_different_device_creates_fresh_ids_and_centered_defaults() {
    let dir = temp_dir("rebind");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();

    let first = store.bind("A", 2, 0).expect("bind A");
    let first_ids: Vec<SourceId> = first.channel_map.iter().map(|e| e.source_id).collect();

    let second = store.bind("B", 2, 1).expect("explicit rebind allowed");
    let second_ids: Vec<SourceId> = second.channel_map.iter().map(|e| e.source_id).collect();

    assert_eq!(second.device_id.as_deref(), Some("B"));
    assert_eq!(second.input_channels, Some(2));
    assert_ne!(first_ids, second_ids, "a new binding mints fresh source ids");
    assert!(second.source_defaults.iter().all(|d| d.pan == 0.0));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn same_device_relayout_retains_source_ids() {
    let dir = temp_dir("same-relayout");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();

    let first = store.bind("A", 2, 0).expect("bind A");
    let first_ids: Vec<SourceId> = first.channel_map.iter().map(|e| e.source_id).collect();

    let second = store.bind("A", 2, 1).expect("same layout retains ids");
    let second_ids: Vec<SourceId> = second.channel_map.iter().map(|e| e.source_id).collect();

    assert_eq!(first_ids, second_ids, "the same physical map keeps its source ids");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn verify_layout_rejects_a_different_device_without_minting_ids() {
    let dir = temp_dir("verify-wrong-device");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 2, 0).expect("bind A");
    let before = store.committed().clone();

    let error = store
        .verify_layout("B", 2, 1)
        .expect_err("a different device is not a same-device verification");
    assert!(matches!(error, ConfigError::DeviceMismatch { .. }));
    assert_eq!(store.committed(), &before);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn committed_revision_and_settings_revision_stay_in_sync() {
    let dir = temp_dir("revision-sync");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    store
        .commit(0, candidate(Some("dev"), Some(1), map.clone(), centered(&map)))
        .unwrap();
    assert_eq!(store.revision(), store.committed().settings_revision.get());

    let reopened = ConfigStore::open(&path).unwrap();
    assert_eq!(reopened.revision(), 1);
    assert_eq!(reopened.revision(), reopened.committed().settings_revision.get());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn revision_overflow_is_rejected_before_any_io() {
    let dir = temp_dir("overflow");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.revision = u64::MAX;
    let map = vec![entry(0, 0x11)];

    let error = store
        .commit(u64::MAX, candidate(Some("dev"), Some(1), map.clone(), centered(&map)))
        .expect_err("an exhausted counter must reject");
    assert!(matches!(error, ConfigError::RevisionExhausted));
    assert!(!path.exists(), "checked overflow must reject before any write");

    fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// durability / transactional write
// ---------------------------------------------------------------------------------------------

#[test]
fn pre_install_write_failures_leave_file_memory_and_revision_unchanged() {
    // Every failure stage before the rename leaves the prior file, memory committed value, and the
    // revision untouched, and never leaves a temp file behind.
    let stages = [
        WriteStage::CreateTemp,
        WriteStage::Write,
        WriteStage::SyncFile,
        WriteStage::Rename,
        #[cfg(unix)]
        WriteStage::OpenDir,
    ];

    for stage in stages {
        let dir = temp_dir(&format!("preinstall-{stage:?}"));
        let path = dir.join("config.json");
        let mut store = ConfigStore::open(&path).unwrap();

        // Establish a committed baseline.
        let base_map = vec![entry(0, 0x11)];
        store
            .commit(0, candidate(Some("dev"), Some(1), base_map.clone(), centered(&base_map)))
            .expect("baseline commit");
        let bytes_before = fs::read(&path).unwrap();
        let committed_before = store.committed().clone();
        assert_eq!(store.revision(), 1);

        // A candidate that would change the stored defaults.
        let next = candidate(
            Some("dev"),
            Some(1),
            base_map.clone(),
            vec![SourcePanDefault { source_id: sid(0x11), pan: 0.5 }],
        );
        inject(&mut store, stage);
        let error = store.commit(1, next).expect_err("injected pre-install failure");

        match error {
            ConfigError::Io(_) => {}
            other => panic!("stage {stage:?} expected Io, got {other:?}"),
        }
        assert_eq!(store.revision(), 1, "stage {stage:?} revision must be unchanged");
        assert_eq!(store.committed(), &committed_before, "stage {stage:?} memory unchanged");
        assert_eq!(fs::read(&path).unwrap(), bytes_before, "stage {stage:?} file unchanged");
        assert!(
            only_expected_files(&dir, "config.json"),
            "stage {stage:?} must not leave a temp file"
        );

        fs::remove_dir_all(&dir).ok();
    }
}

#[test]
#[cfg(unix)]
fn rename_installed_directory_sync_failure_adopts_candidate_and_reports_uncertain() {
    // Unix-only: the directory-handle sync stage (`WriteStage::SyncDir`) exists only where directory
    // handles are available. Windows durability has not been qualified, so this is not asserted
    // there.
    // The file is installed by the rename, but the parent-directory sync fails. The store must
    // adopt the installed revision (not roll back) and return DurabilityUncertain, with the file
    // bytes holding the new candidate and the revision advanced.
    let dir = temp_dir("durability-uncertain");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let base_map = vec![entry(0, 0x11)];
    store
        .commit(0, candidate(Some("dev"), Some(1), base_map.clone(), centered(&base_map)))
        .expect("baseline commit");

    let next = candidate(
        Some("dev"),
        Some(1),
        base_map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: 0.75 }],
    );
    inject(&mut store, WriteStage::SyncDir);
    let error = store.commit(1, next).expect_err("post-install sync failure is uncertain");

    match error {
        ConfigError::DurabilityUncertain { revision, .. } => assert_eq!(revision, 2),
        other => panic!("expected DurabilityUncertain, got {other:?}"),
    }

    // Memory adopted the installed revision; disk holds the installed candidate.
    assert_eq!(store.revision(), 2);
    assert_eq!(store.committed().settings_revision, SettingsRevision(2));
    assert_eq!(
        store
            .committed()
            .source_defaults
            .iter()
            .find(|d| d.source_id == sid(0x11))
            .unwrap()
            .pan,
        0.75
    );

    let on_disk = ConfigStore::open(&path).expect("reopen installed file");
    assert_eq!(on_disk.revision(), 2);
    assert_eq!(
        on_disk
            .committed()
            .source_defaults
            .iter()
            .find(|d| d.source_id == sid(0x11))
            .unwrap()
            .pan,
        0.75
    );

    fs::remove_dir_all(&dir).ok();
}

/// Whether `dir` contains exactly `{name}` (no leftover temp files).
fn only_expected_files(dir: &std::path::Path, name: &str) -> bool {
    let mut entries: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    entries == vec![name.to_string()]
}

// ---------------------------------------------------------------------------------------------
// map/default validation
// ---------------------------------------------------------------------------------------------

#[test]
fn valid_nonadjacent_higher_index_left_pair_persists_with_matching_default() {
    let dir = temp_dir("valid-pair");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();

    let map = vec![
        ChannelMapEntry {
            physical_index: 0,
            source_id: sid(0x11),
            stereo_pair: Some(sid(0x44)),
            stereo_side: Some(StereoSide::Right),
            role: SourceRole::InputChannel,
        },
        entry(1, 0x22),
        entry(2, 0x33),
        ChannelMapEntry {
            physical_index: 3,
            source_id: sid(0x44),
            stereo_pair: Some(sid(0x11)),
            stereo_side: Some(StereoSide::Left),
            role: SourceRole::InputChannel,
        },
    ];
    let defaults = vec![
        SourcePanDefault { source_id: sid(0x11), pan: 0.25 },
        SourcePanDefault { source_id: sid(0x22), pan: 0.0 },
        SourcePanDefault { source_id: sid(0x33), pan: -0.5 },
        SourcePanDefault { source_id: sid(0x44), pan: 0.25 },
    ];
    store
        .commit(0, candidate(Some("dev"), Some(4), map, defaults))
        .expect("an explicit non-adjacent pair is valid");

    let reopened = ConfigStore::open(&path).unwrap();
    let higher_left = reopened
        .committed()
        .channel_map
        .iter()
        .find(|e| e.physical_index == 3)
        .expect("index 3 present");
    assert_eq!(higher_left.stereo_side, Some(StereoSide::Left));
    let paired_default = reopened
        .committed()
        .source_defaults
        .iter()
        .find(|d| d.source_id == sid(0x44))
        .expect("paired default present");
    assert_eq!(paired_default.pan, 0.25);
    let independent = reopened
        .committed()
        .source_defaults
        .iter()
        .find(|d| d.source_id == sid(0x33))
        .expect("independent default present");
    assert_eq!(independent.pan, -0.5);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn invalid_pan_or_metadata_fails_closed() {
    let dir = temp_dir("invalid");
    let path = dir.join("config.json");

    // The store is unbound; each candidate carries a consistent (device, count) binding so the
    // rejection is exercised by the pan/metadata/validation rule under test, not the binding rule.
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    let nan = candidate(
        Some("dev"),
        Some(1),
        map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: f32::NAN }],
    );
    assert!(store.commit(0, nan).is_err(), "NaN pan must fail closed");

    let out_of_bounds = candidate(
        Some("dev"),
        Some(1),
        map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: 1.5 }],
    );
    assert!(store.commit(0, out_of_bounds).is_err(), "out-of-bounds pan must fail closed");

    let dup = candidate(Some("dev"), Some(2), vec![entry(0, 0x11), entry(1, 0x11)], vec![]);
    assert!(store.commit(0, dup).is_err(), "duplicate source ids must fail closed");

    let mut non_reciprocal = vec![entry(0, 0x11), entry(1, 0x22)];
    non_reciprocal[0].stereo_pair = Some(sid(0x22));
    non_reciprocal[0].stereo_side = Some(StereoSide::Left);
    non_reciprocal[1].stereo_pair = Some(sid(0x11));
    non_reciprocal[1].stereo_side = Some(StereoSide::Left);
    let bad_pair = candidate(
        Some("dev"),
        Some(2),
        non_reciprocal,
        vec![
            SourcePanDefault { source_id: sid(0x11), pan: 0.0 },
            SourcePanDefault { source_id: sid(0x22), pan: 0.0 },
        ],
    );
    assert!(store.commit(0, bad_pair).is_err(), "equal sides must fail closed");

    let incomplete = candidate(Some("dev"), Some(1), map.clone(), vec![]);
    assert!(store.commit(0, incomplete).is_err(), "an incomplete defaults vector must fail closed");

    // Physical index 99 in a 2-channel layout.
    let bad_index = candidate(Some("dev"), Some(2), vec![entry(0, 0x11), entry(99, 0x22)], vec![]);
    assert!(store.commit(0, bad_index).is_err(), "an out-of-bounds index must fail closed");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn apply_source_links_sets_opposite_sides_and_recenters_affected_defaults() {
    let map = vec![entry(0, 0x11), entry(1, 0x22), entry(2, 0x33)];
    let defaults = vec![
        SourcePanDefault { source_id: sid(0x11), pan: 0.5 },
        SourcePanDefault { source_id: sid(0x22), pan: -0.25 },
        SourcePanDefault { source_id: sid(0x33), pan: -0.5 },
    ];

    let (linked, recentered) =
        apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x33))]).expect("valid link");

    assert_eq!(linked[0].stereo_side, Some(StereoSide::Left));
    assert_eq!(linked[0].stereo_pair, Some(sid(0x33)));
    assert_eq!(linked[2].stereo_side, Some(StereoSide::Right));
    assert_eq!(linked[2].stereo_pair, Some(sid(0x11)));
    assert_eq!(linked[1].stereo_pair, None);

    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x11)).unwrap().pan, 0.0);
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x33)).unwrap().pan, 0.0);
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x22)).unwrap().pan, -0.25);

    let (unlinked, recentered) = apply_source_links(&linked, &recentered, &[]).expect("unlink");
    assert!(unlinked.iter().all(|e| e.stereo_pair.is_none() && e.stereo_side.is_none()));
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x11)).unwrap().pan, 0.0);
}

#[test]
fn reversing_a_pair_recenters_both_members_and_leaves_unrelated_defaults() {
    // A nonzero balance shared by the pair, plus an unrelated source's default.
    let map = vec![
        ChannelMapEntry {
            physical_index: 0,
            source_id: sid(0x11),
            stereo_pair: Some(sid(0x22)),
            stereo_side: Some(StereoSide::Left),
            role: SourceRole::InputChannel,
        },
        ChannelMapEntry {
            physical_index: 1,
            source_id: sid(0x22),
            stereo_pair: Some(sid(0x11)),
            stereo_side: Some(StereoSide::Right),
            role: SourceRole::InputChannel,
        },
        entry(2, 0x33),
    ];
    let defaults = vec![
        SourcePanDefault { source_id: sid(0x11), pan: 0.4 },
        SourcePanDefault { source_id: sid(0x22), pan: 0.4 },
        SourcePanDefault { source_id: sid(0x33), pan: -0.5 },
    ];

    // Reverse: same peer, swapped sides.
    let (reversed, recentered) =
        apply_source_links(&map, &defaults, &[(sid(0x22), sid(0x11))]).expect("reversal");

    assert_eq!(reversed[0].stereo_pair, Some(sid(0x22)));
    assert_eq!(reversed[0].stereo_side, Some(StereoSide::Right));
    assert_eq!(reversed[1].stereo_side, Some(StereoSide::Left));
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x11)).unwrap().pan, 0.0);
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x22)).unwrap().pan, 0.0);
    assert_eq!(
        recentered.iter().find(|d| d.source_id == sid(0x33)).unwrap().pan,
        -0.5,
        "an unrelated default is untouched by a reversal"
    );
}

#[test]
fn apply_source_links_rejects_overlap_and_unknown_ids() {
    let map = vec![entry(0, 0x11), entry(1, 0x22), entry(2, 0x33)];
    let defaults = centered(&map);

    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x22)), (sid(0x11), sid(0x33))]).is_err());
    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x99))]).is_err());
    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x11))]).is_err());
}

// ---------------------------------------------------------------------------------------------
// prepare_binding / commit_prepared_binding
// ---------------------------------------------------------------------------------------------

#[test]
fn prepare_binding_is_pure_and_retains_compatible_ids_map_labels_and_defaults() {
    // A committed same-device binding: preparation writes nothing, advances no revision, mutates
    // no memory, and returns a candidate carrying the exact committed identity.
    let dir = temp_dir("prepare-compatible");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let bound = store.bind("A", 2, 0).expect("bind A");
    // Commit a source-bound label and a non-center default through the ordinary path.
    let ids: Vec<SourceId> = bound.channel_map.iter().map(|e| e.source_id).collect();
    let mut labeled = bound.clone();
    labeled.labels = vec![(ids[0], "Kick".to_string())];
    labeled.source_defaults = vec![
        SourcePanDefault { source_id: ids[0], pan: -0.4 },
        SourcePanDefault { source_id: ids[1], pan: 0.3 },
    ];
    store.commit(1, labeled).expect("commit labeled defaults");
    let bytes_before = fs::read(&path).unwrap();
    let committed_before = store.committed().clone();
    let revision_before = store.revision();

    let prepared = store.prepare_binding("A", 2).expect("prepare retains the binding");

    assert_eq!(prepared.device_id.as_deref(), Some("A"));
    assert_eq!(prepared.input_channels, Some(2));
    assert_eq!(
        prepared.channel_map, committed_before.channel_map,
        "compatible preparation retains ids/map"
    );
    assert_eq!(
        prepared.source_defaults, committed_before.source_defaults,
        "compatible preparation retains defaults"
    );
    assert_eq!(prepared.labels, committed_before.labels, "labels retained");
    assert_eq!(prepared.labels.len(), 1, "the source-bound label survives");
    assert_eq!(
        prepared.settings_revision, SettingsRevision(revision_before),
        "prepared revision is the committed save base, not a draft"
    );

    // Pure: no file, memory, or revision change.
    assert_eq!(fs::read(&path).unwrap(), bytes_before, "no write on prepare");
    assert_eq!(store.committed(), &committed_before, "memory unchanged");
    assert_eq!(store.revision(), revision_before, "revision unchanged");
    assert!(only_expected_files(&dir, "config.json"), "no temp file left");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn repeat_prepare_binding_returns_stable_ids() {
    // A compatible preparation is a pure projection of committed identity, so repeating it yields
    // the same ids/defaults without touching committed state. (Stable ids for a *new* pending
    // binding are the native shell's cache responsibility, not this core helper's.)
    let dir = temp_dir("prepare-repeat");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 3, 0).expect("bind A");
    let bytes_before = fs::read(&path).unwrap();

    let first = store.prepare_binding("A", 3).expect("first prepare");
    let second = store.prepare_binding("A", 3).expect("second prepare");

    assert_eq!(
        first.channel_map, second.channel_map,
        "repeat preparation must not churn source ids"
    );
    assert_eq!(first.source_defaults, second.source_defaults);
    assert_eq!(store.revision(), 1, "no revision change across reads");
    assert_eq!(fs::read(&path).unwrap(), bytes_before, "no write across reads");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepare_binding_for_a_new_or_incompatible_binding_mints_fresh_ids_and_clears_labels() {
    let dir = temp_dir("prepare-new");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let existing = store.bind("A", 2, 0).expect("bind A");
    let existing_ids: Vec<SourceId> = existing.channel_map.iter().map(|e| e.source_id).collect();

    // Commit a real source-bound label plus a non-center default so clearing is observable.
    let mut labeled = existing.clone();
    labeled.labels = vec![(existing_ids[0], "Kick".to_string())];
    labeled.source_defaults = vec![
        SourcePanDefault { source_id: existing_ids[0], pan: -0.4 },
        SourcePanDefault { source_id: existing_ids[1], pan: 0.3 },
    ];
    store.commit(1, labeled).expect("commit labeled binding");
    assert_eq!(store.committed().labels.len(), 1, "a real label is committed");
    let bytes_before = fs::read(&path).unwrap();

    // A different device: fresh mono ids, centered defaults, no source-bound labels.
    let prepared = store.prepare_binding("B", 2).expect("prepare a new device");
    let prepared_ids: Vec<SourceId> = prepared.channel_map.iter().map(|e| e.source_id).collect();
    assert_eq!(prepared.device_id.as_deref(), Some("B"));
    assert_ne!(prepared_ids, existing_ids, "a new binding mints fresh ids");
    assert!(
        prepared.channel_map.iter().all(|e| {
            e.stereo_pair.is_none() && e.stereo_side.is_none() && e.role == SourceRole::InputChannel
        }),
        "fresh map is mono/unpaired"
    );
    assert!(
        prepared.source_defaults.iter().all(|d| d.pan == 0.0),
        "fresh defaults are centered"
    );
    assert!(
        prepared.labels.is_empty(),
        "the committed source-bound label is cleared on a new binding"
    );

    // An incompatible layout on the SAME device also mints fresh identity and clears labels.
    let incompatible = store.prepare_binding("A", 4).expect("prepare a new layout");
    let incompatible_ids: Vec<SourceId> =
        incompatible.channel_map.iter().map(|e| e.source_id).collect();
    assert_ne!(
        incompatible_ids, existing_ids,
        "a layout change under the same uid is a fresh binding"
    );
    assert!(
        incompatible.labels.is_empty(),
        "the committed label is cleared on an incompatible layout too"
    );

    assert_eq!(store.committed().device_id.as_deref(), Some("A"), "committed untouched");
    assert_eq!(store.revision(), 2, "prepare never advances the revision");
    assert_eq!(fs::read(&path).unwrap(), bytes_before, "prepare never writes");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepare_binding_legacy_compatible_retains_ids_labels_and_unresolved_links() {
    // A migrated v1 (unknown layout) with an unresolved pair. Same-uid preparation retains ids,
    // labels, and the unresolved link in the candidate so the operator can resolve it.
    let dir = temp_dir("prepare-legacy");
    let path = dir.join("config.json");
    let mut map = vec![entry(0, 0x11), entry(1, 0x22)];
    map[0].stereo_pair = Some(sid(0x22));
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: None,
        channel_map: map,
        labels: vec![(sid(0x11), "Kick".to_string())],
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");
    let original_bytes = fs::read(&path).unwrap();

    let store = ConfigStore::open(&path).expect("legacy load");
    let ids_before: Vec<SourceId> =
        store.committed().channel_map.iter().map(|e| e.source_id).collect();

    let prepared = store
        .prepare_binding("dev-A", 2)
        .expect("same-device legacy preparation is allowed");
    let ids_after: Vec<SourceId> = prepared.channel_map.iter().map(|e| e.source_id).collect();

    assert_eq!(ids_before, ids_after, "legacy ids retained");
    assert_eq!(prepared.labels.len(), 1, "legacy labels retained");
    assert_eq!(
        prepared.channel_map[0].stereo_pair,
        Some(sid(0x22)),
        "the unresolved link is preserved in the candidate"
    );
    assert!(
        validate_config(&prepared).is_err(),
        "the unresolved candidate is intentionally not yet canonical"
    );
    assert_eq!(fs::read(&path).unwrap(), original_bytes, "prepare never writes");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepared_unresolved_legacy_candidate_is_rejected_by_commit_until_resolved() {
    // Preparation is allowed to surface an unresolved legacy candidate, but the explicit commit is
    // fully validated: it must reject the unresolved candidate and only accept a resolved one.
    let dir = temp_dir("commit-prepared-unresolved");
    let path = dir.join("config.json");
    let mut map = vec![entry(0, 0x11), entry(1, 0x22)];
    map[0].stereo_pair = Some(sid(0x22));
    let v1 = Config {
        version: 1,
        device_id: Some("dev-A".to_string()),
        selected_interface: None,
        channel_map: map,
        labels: Vec::new(),
    };
    fs::write(&path, serde_json::to_vec(&v1).unwrap()).expect("write v1");
    let original_bytes = fs::read(&path).unwrap();

    let mut store = ConfigStore::open(&path).expect("legacy load");
    let unresolved = store.prepare_binding("dev-A", 2).expect("prepare allowed");
    assert!(
        matches!(
            store.commit_prepared_binding(0, unresolved),
            Err(ConfigError::Corrupt)
        ),
        "an unresolved candidate cannot be installed"
    );
    assert_eq!(store.revision(), 0);
    assert_eq!(fs::read(&path).unwrap(), original_bytes, "no write on rejection");

    // After the operator resolves the sides, the same prepared identity commits.
    let (resolved_map, resolved_defaults) = apply_source_links(
        &store.committed().channel_map,
        &store.committed().source_defaults,
        &[(sid(0x11), sid(0x22))],
    )
    .expect("resolve the pair");
    let mut resolved = store.prepare_binding("dev-A", 2).expect("prepare resolved");
    resolved.channel_map = resolved_map;
    resolved.source_defaults = resolved_defaults;
    store
        .commit_prepared_binding(0, resolved)
        .expect("resolved prepared candidate installs");

    let reopened = ConfigStore::open(&path).expect("reopen");
    assert_eq!(reopened.revision(), 1);
    assert_eq!(reopened.committed().channel_map[0].stereo_side, Some(StereoSide::Left));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepared_commit_installs_exact_supplied_ids_and_final_custom_pan_in_one_write() {
    // Prepare a fresh binding, then install a fully-formed candidate that carries preview ids plus a
    // final custom pan. The exact supplied ids/defaults must be what reopens.
    //
    // "One write" is established by source inspection: `commit_prepared_binding` delegates to the
    // same `persist_candidate`/`write_atomic` path as the ordinary commit, so there is exactly one
    // atomic replacement and no second map generation. This test does not count writes; it observes
    // a single revision advance and no leftover temp file, which is consistent with that shared path.
    let dir = temp_dir("commit-prepared-exact");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();

    let prepared = store.prepare_binding("A", 2).expect("prepare new binding");
    let preview_ids: Vec<SourceId> = prepared.channel_map.iter().map(|e| e.source_id).collect();

    let mut final_candidate = prepared.clone();
    final_candidate.source_defaults = vec![
        SourcePanDefault { source_id: preview_ids[0], pan: -0.25 },
        SourcePanDefault { source_id: preview_ids[1], pan: -0.25 },
    ];
    let saved = store
        .commit_prepared_binding(0, final_candidate)
        .expect("prepared install");
    assert_eq!(saved.settings_revision, SettingsRevision(1));
    assert_eq!(store.revision(), 1, "exactly one revision advance");
    assert!(
        only_expected_files(&dir, "config.json"),
        "no leftover temp file from the atomic write"
    );

    let reopened = ConfigStore::open(&path).expect("reopen");
    let reopened_ids: Vec<SourceId> =
        reopened.committed().channel_map.iter().map(|e| e.source_id).collect();
    assert_eq!(reopened_ids, preview_ids, "preview ids are preserved on save/reopen");
    let pan = reopened
        .committed()
        .source_defaults
        .iter()
        .find(|d| d.source_id == preview_ids[0])
        .unwrap()
        .pan;
    assert_eq!(pan, -0.25, "the final custom payload pan persists");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepared_commit_replaces_an_existing_incompatible_binding_where_ordinary_commit_rejects() {
    // Explicit prepared commit is not binding-restrictive: it can replace a different device where
    // the ordinary commit returns DeviceMismatch. Consent/lifecycle are the native caller's job.
    let dir = temp_dir("commit-prepared-replace");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 2, 0).expect("bind A");

    let replacement = store.prepare_binding("B", 2).expect("prepare replacement");
    let ordinary = replacement.clone();
    assert!(
        matches!(store.commit(1, ordinary), Err(ConfigError::DeviceMismatch { .. })),
        "ordinary commit stays binding-restrictive"
    );
    assert_eq!(store.revision(), 1, "rejected ordinary commit wrote nothing");

    let saved = store
        .commit_prepared_binding(1, replacement)
        .expect("explicit prepared replacement installs");
    assert_eq!(saved.device_id.as_deref(), Some("B"));
    assert_eq!(store.revision(), 2);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn prepared_commit_rejects_stale_revision_exhaustion_and_io_without_side_effects() {
    let dir = temp_dir("commit-prepared-failures");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    store.bind("A", 1, 0).expect("bind baseline");
    let prepared = store.prepare_binding("A", 1).expect("prepare");
    let bytes_before = fs::read(&path).unwrap();

    // Stale expected revision.
    assert!(matches!(
        store.commit_prepared_binding(7, prepared.clone()),
        Err(ConfigError::RevisionConflict { expected: 7, found: 1 })
    ));
    assert_eq!(store.revision(), 1);
    assert_eq!(fs::read(&path).unwrap(), bytes_before);

    // Exhausted counter rejects before any I/O.
    store.revision = u64::MAX;
    assert!(matches!(
        store.commit_prepared_binding(u64::MAX, prepared.clone()),
        Err(ConfigError::RevisionExhausted)
    ));
    store.revision = 1;

    // Pre-install I/O failure leaves memory/revision/file untouched.
    let conf = dir.join("conf");
    fs::create_dir_all(&conf).unwrap();
    let broken_path = conf.join("config.json");
    let mut broken = ConfigStore::open(&broken_path).unwrap();
    broken.bind("A", 1, 0).expect("bind broken baseline");
    let broken_prepared = broken.prepare_binding("A", 1).expect("prepare");
    let broken_revision = broken.revision();
    fs::remove_dir_all(&conf).unwrap();
    fs::write(&conf, b"not a directory").unwrap();
    let error = broken
        .commit_prepared_binding(broken_revision, broken_prepared)
        .expect_err("unwritable path must fail");
    assert!(matches!(error, ConfigError::Io(_)), "expected Io, got {error:?}");
    assert_eq!(broken.revision(), broken_revision, "revision not advanced on pre-install failure");
    fs::remove_file(&conf).ok();

    fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn prepared_commit_post_install_sync_failure_adopts_and_reports_uncertain() {
    // The explicit commit reuses the same atomic writer and its DurabilityUncertain distinction.
    let dir = temp_dir("commit-prepared-uncertain");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let prepared = store.prepare_binding("A", 1).expect("prepare");
    inject(&mut store, WriteStage::SyncDir);

    let error = store
        .commit_prepared_binding(0, prepared)
        .expect_err("post-install sync failure is uncertain");
    match error {
        ConfigError::DurabilityUncertain { revision, .. } => assert_eq!(revision, 1),
        other => panic!("expected DurabilityUncertain, got {other:?}"),
    }
    assert_eq!(store.revision(), 1, "installed revision adopted");
    assert_eq!(store.committed().settings_revision, SettingsRevision(1));
    assert!(store.committed().device_id.as_deref() == Some("A"));

    let on_disk = ConfigStore::open(&path).expect("reopen installed file");
    assert_eq!(on_disk.revision(), 1);
    assert_eq!(on_disk.binding(), Some("A"));

    fs::remove_dir_all(&dir).ok();
}
