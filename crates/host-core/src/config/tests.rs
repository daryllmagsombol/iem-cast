//! Transactional config store tests.
//!
//! Every test uses its own unique temp directory and removes it on the way out; no shared path and
//! no broad `rm`. The store path lives under that directory. No test opens real hardware.

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
    // Opening must not create, truncate, or otherwise touch the file.
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
fn v1_unresolved_pair_loads_for_ui_but_cannot_be_committed() {
    let dir = temp_dir("v1-unresolved");
    let path = dir.join("config.json");
    let mut map = vec![entry(0, 0x11), entry(1, 0x22)];
    // Legacy pair: a peer id but no explicit side (unresolved).
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
    assert!(
        store.has_unresolved_pairs(),
        "the unresolved legacy pair must be visible so the UI can require a side or unlink"
    );

    // Saving the same unresolved map must fail closed rather than infer a side.
    let unresolved = store.committed().channel_map.clone();
    let defaults = store.committed().source_defaults.clone();
    let cand = candidate(Some("dev-A"), None, unresolved, defaults);
    assert!(
        matches!(store.commit(0, cand), Err(ConfigError::Corrupt)),
        "an unresolved pair cannot be persisted"
    );

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

    let cand = candidate(Some("B"), Some(2), store.committed().channel_map.clone(), centered(&store.committed().channel_map));
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
fn commit_with_stale_revision_is_rejected_and_memory_unchanged() {
    let dir = temp_dir("stale");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    let cand = candidate(None, Some(1), map.clone(), centered(&map));
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

    // Replace the parent directory with a regular file so the temp-file write cannot succeed.
    fs::remove_dir_all(&conf).unwrap();
    fs::write(&conf, b"not a directory").unwrap();

    let map = vec![entry(0, 0x11)];
    let cand = candidate(None, Some(1), map.clone(), centered(&map));
    let error = store.commit(0, cand).expect_err("unwritable path must fail");
    assert!(matches!(error, ConfigError::Io(_)), "expected Io, got {error:?}");
    assert_eq!(store.revision(), 0, "revision must not advance on a failed save");
    assert!(store.committed().channel_map.is_empty());

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
fn committed_revision_and_settings_revision_stay_in_sync() {
    let dir = temp_dir("revision-sync");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    store
        .commit(0, candidate(None, Some(1), map.clone(), centered(&map)))
        .unwrap();
    assert_eq!(store.revision(), store.committed().settings_revision.get());

    // Reopening trusts the persisted settingsRevision as the current revision.
    let reopened = ConfigStore::open(&path).unwrap();
    assert_eq!(reopened.revision(), 1);
    assert_eq!(reopened.revision(), reopened.committed().settings_revision.get());

    fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------------
// map/default validation
// ---------------------------------------------------------------------------------------------

#[test]
fn valid_nonadjacent_higher_index_left_pair_persists_with_matching_default() {
    let dir = temp_dir("valid-pair");
    let path = dir.join("config.json");
    let mut store = ConfigStore::open(&path).unwrap();

    // Left member has the higher physical index (3) and the pair is non-adjacent: explicit choice.
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
        .commit(0, candidate(None, Some(4), map, defaults))
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
    // Independent non-paired gains/defaults are out of scope for validation.
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

    // Non-finite default pan.
    let mut store = ConfigStore::open(&path).unwrap();
    let map = vec![entry(0, 0x11)];
    let nan = candidate(
        None,
        Some(1),
        map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: f32::NAN }],
    );
    assert!(store.commit(0, nan).is_err(), "NaN pan must fail closed");

    // Out-of-bounds default pan.
    let out_of_bounds = candidate(
        None,
        Some(1),
        map.clone(),
        vec![SourcePanDefault { source_id: sid(0x11), pan: 1.5 }],
    );
    assert!(store.commit(0, out_of_bounds).is_err(), "out-of-bounds pan must fail closed");

    // Duplicate source ids.
    let dup = candidate(None, Some(2), vec![entry(0, 0x11), entry(1, 0x11)], vec![]);
    assert!(store.commit(0, dup).is_err(), "duplicate source ids must fail closed");

    // Non-reciprocal pair.
    let mut non_reciprocal = vec![entry(0, 0x11), entry(1, 0x22)];
    non_reciprocal[0].stereo_pair = Some(sid(0x22));
    non_reciprocal[0].stereo_side = Some(StereoSide::Left);
    non_reciprocal[1].stereo_pair = Some(sid(0x11));
    non_reciprocal[1].stereo_side = Some(StereoSide::Left); // not opposite
    let bad_pair = candidate(
        None,
        Some(2),
        non_reciprocal,
        vec![
            SourcePanDefault { source_id: sid(0x11), pan: 0.0 },
            SourcePanDefault { source_id: sid(0x22), pan: 0.0 },
        ],
    );
    assert!(store.commit(0, bad_pair).is_err(), "equal sides must fail closed");

    // Missing default for a mapped source.
    let incomplete = candidate(None, Some(1), map.clone(), vec![]);
    assert!(store.commit(0, incomplete).is_err(), "an incomplete defaults vector must fail closed");

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

    // Affected members reset to center; untouched members keep their defaults.
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x11)).unwrap().pan, 0.0);
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x33)).unwrap().pan, 0.0);
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x22)).unwrap().pan, -0.25);

    // An empty link set unlinks everything and recenters the previously-paired members.
    let (unlinked, recentered) = apply_source_links(&linked, &recentered, &[]).expect("unlink");
    assert!(unlinked.iter().all(|e| e.stereo_pair.is_none() && e.stereo_side.is_none()));
    assert_eq!(recentered.iter().find(|d| d.source_id == sid(0x11)).unwrap().pan, 0.0);
}

#[test]
fn apply_source_links_rejects_overlap_and_unknown_ids() {
    let map = vec![entry(0, 0x11), entry(1, 0x22), entry(2, 0x33)];
    let defaults = centered(&map);

    // The same source appears in two links.
    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x22)), (sid(0x11), sid(0x33))]).is_err());
    // A link references an unknown source.
    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x99))]).is_err());
    // A self-pair.
    assert!(apply_source_links(&map, &defaults, &[(sid(0x11), sid(0x11))]).is_err());
}
