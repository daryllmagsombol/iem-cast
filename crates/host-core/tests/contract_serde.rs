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
