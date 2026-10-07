//! Shared wire fixtures.
//!
//! The JSON files in `crates/host-core/fixtures/` are the single source of truth for both the Rust
//! and TypeScript tests. This module embeds and parses them.

use crate::contract::{CatalogSnapshot, EnvelopeV1, ListenArm, MixApplied, MixPatch};

/// Wire JSON for a `mix.patch` envelope.
pub const MIX_PATCH_JSON: &str = include_str!("../fixtures/mix.patch.json");
/// Wire JSON for a `catalog.snapshot` payload.
pub const CATALOG_SNAPSHOT_JSON: &str = include_str!("../fixtures/catalog.snapshot.json");
/// Wire JSON for a `mix.applied` payload.
pub const MIX_APPLIED_JSON: &str = include_str!("../fixtures/mix.applied.json");
/// Wire JSON for a `listen.arm` payload.
pub const ARM_JSON: &str = include_str!("../fixtures/arm.json");

/// Parse the `mix.patch` fixture envelope.
pub fn mix_patch() -> EnvelopeV1<MixPatch> {
    serde_json::from_str(MIX_PATCH_JSON).expect("valid mix.patch fixture")
}

/// Parse the `catalog.snapshot` fixture payload.
pub fn catalog_snapshot() -> CatalogSnapshot {
    serde_json::from_str(CATALOG_SNAPSHOT_JSON).expect("valid catalog.snapshot fixture")
}

/// Parse the `mix.applied` fixture payload.
pub fn mix_applied() -> MixApplied {
    serde_json::from_str(MIX_APPLIED_JSON).expect("valid mix.applied fixture")
}

/// Parse the `listen.arm` fixture payload.
pub fn arm() -> ListenArm {
    serde_json::from_str(ARM_JSON).expect("valid listen.arm fixture")
}
