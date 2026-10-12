// crates/host-core/tests/contract_serde.rs
use host_core::contract::*;
use host_core::ids::*;
use uuid::Uuid;

fn uuid(s: &str) -> Uuid { Uuid::parse_str(s).unwrap() }

#[test]
fn counters_serialize_as_decimal_strings_and_opaque_ids_as_uuid_strings() {
    let env = EnvelopeV1 {
        v: 1,
        kind: "mix.patch".to_string(),
        request_id: RequestId("req-1".to_string()),
        host_epoch: HostEpoch(uuid("11111111-1111-4111-8111-111111111111")),
        session_epoch: Some(SessionEpoch(uuid("22222222-2222-4222-8222-222222222222"))),
        payload: MixPatch {
            base_revision: MixRevision(120),
            catalog_revision: CatalogRevision(34),
            sources: SourceGainMatrix::from_slice(&[SourceGain {
                source_id: SourceId(uuid("33333333-3333-4333-8333-333333333333")),
                gain_db: -12.0,
                muted: false,
                pan: 0.0,
            }]),
            master_db: -6.0,
            master_muted: false,
        },
    };
    let j = serde_json::to_value(&env).unwrap();
    assert_eq!(j["type"], "mix.patch");
    assert_eq!(j["payload"]["baseRevision"], "120");
    assert_eq!(j["payload"]["catalogRevision"], "34");
    assert_eq!(j["hostEpoch"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(j["payload"]["sources"][0]["sourceId"], "33333333-3333-4333-8333-333333333333");
    assert_eq!(j["payload"]["sources"][0]["gainDb"], -12.0);
    assert_eq!(j["payload"]["sources"][0]["muted"], false);
    let s = serde_json::to_string(&env).unwrap();
    assert!(s.contains("\"baseRevision\":\"120\""));
}

#[test]
fn frame_count_wire_is_decimal_and_rejects_above_u32_range() {
    assert_eq!(serde_json::to_string(&FrameCountWire(240)).unwrap(), "\"240\"");
    assert!(serde_json::from_str::<FrameCountWire>("\"4294967296\"").is_err());
    assert!(serde_json::from_str::<FrameCountWire>("\"1.5\"").is_err());
}

// ---------------------------------------------------------------------------------------------
// Channel pan / stereo balance (Task 1 contracts)
// ---------------------------------------------------------------------------------------------

#[test]
fn omitted_pan_decodes_as_center() {
    let g: SourceGain = serde_json::from_str(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false}"#,
    )
    .unwrap();
    assert_eq!(g.pan, 0.0);
}

#[test]
fn null_pan_is_rejected() {
    assert!(serde_json::from_str::<SourceGain>(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false,"pan":null}"#,
    )
    .is_err());
}

#[test]
fn string_pan_is_rejected() {
    assert!(serde_json::from_str::<SourceGain>(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false,"pan":"left"}"#,
    )
    .is_err());
}

#[test]
fn pan_round_trips_on_the_wire() {
    let g = SourceGain {
        source_id: SourceId(uuid("33333333-3333-4333-8333-333333333333")),
        gain_db: -6.0,
        muted: false,
        pan: 0.5,
    };
    let j = serde_json::to_value(g).unwrap();
    assert_eq!(j["pan"], 0.5);
    let back: SourceGain = serde_json::from_value(j).unwrap();
    assert_eq!(back.pan, 0.5);
}

#[test]
fn stereo_side_and_default_pan_round_trip() {
    let s: SourceInfo = serde_json::from_str(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","physicalIndex":0,"label":"C1","role":"inputChannel","authorized":true,"available":true,"stereoPair":null,"stereoSide":"left","defaultPan":0.35}"#,
    )
    .unwrap();
    assert_eq!(s.stereo_side, Some(StereoSide::Left));
    assert_eq!(s.default_pan, 0.35);
}

#[test]
fn legacy_source_info_omits_side_and_defaults_pan() {
    let s: SourceInfo = serde_json::from_str(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","physicalIndex":0,"label":"C1","role":"inputChannel","authorized":true,"available":true,"stereoPair":null}"#,
    )
    .unwrap();
    assert_eq!(s.stereo_side, None);
    assert_eq!(s.default_pan, 0.0);
}

#[test]
fn stereo_side_serializes_camel_case() {
    assert_eq!(serde_json::to_string(&StereoSide::Left).unwrap(), "\"left\"");
    assert_eq!(serde_json::to_string(&StereoSide::Right).unwrap(), "\"right\"");
}

#[test]
fn channel_map_entry_side_defaults_to_none() {
    let e: ChannelMapEntry = serde_json::from_str(
        r#"{"physicalIndex":3,"sourceId":"33333333-3333-4333-8333-333333333333","stereoPair":null,"role":"inputChannel"}"#,
    )
    .unwrap();
    assert_eq!(e.stereo_side, None);
}

#[test]
fn source_pan_default_is_a_camel_case_object_vector() {
    let defaults = vec![SourcePanDefault {
        source_id: SourceId(uuid("33333333-3333-4333-8333-333333333333")),
        pan: -0.25,
    }];
    let j = serde_json::to_value(&defaults).unwrap();
    assert_eq!(j[0]["sourceId"], "33333333-3333-4333-8333-333333333333");
    assert_eq!(j[0]["pan"], -0.25);
}

#[test]
fn config_v2_wire_shape_keeps_revision_as_decimal_string() {
    let cfg = ConfigV2 {
        version: 2,
        device_id: Some("dev-1".to_string()),
        selected_interface: Some("en0".to_string()),
        input_channels: Some(2),
        settings_revision: SettingsRevision(7),
        channel_map: vec![],
        labels: vec![],
        source_defaults: vec![SourcePanDefault {
            source_id: SourceId(uuid("33333333-3333-4333-8333-333333333333")),
            pan: 0.0,
        }],
    };
    let j = serde_json::to_value(&cfg).unwrap();
    assert_eq!(j["version"], 2);
    assert_eq!(j["settingsRevision"], "7");
    assert_eq!(j["inputChannels"], 2);
    assert_eq!(j["sourceDefaults"][0]["pan"], 0.0);
    let back: ConfigV2 = serde_json::from_value(j).unwrap();
    assert_eq!(back, cfg);
}
