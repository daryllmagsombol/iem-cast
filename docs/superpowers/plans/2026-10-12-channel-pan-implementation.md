# IEM Cast — Channel Pan / Stereo Balance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-source pan/balance to each personal mix, explicit desktop-only stereo-pair linking bound to a verified capture device, durable versioned config, and grouped phone controls — without changing the −6 dB headroom, limiter, encoder, or safety gate.

**Architecture:** Rust `host-core` extends `SourceGain`/`SourceInfo`/`ChannelMapEntry` with pan and stereo-side metadata; a canonicalizing pan validator and atomic pair validation live in the control plane; the DSP engine builds a **precomputed bounded routing plan** and adds a 240-frame position ramp, failing closed on omissions. A transactional, device-bound `ConfigV2` store (single profile, no DB) backs defaults. The desktop Tauri shell exposes read/save commands guarded by the actual injected caller window. One shared pure `initialMix(catalog)` builder seeds both phone paths; the UI renders two grouped strips with one shared balance plus mono pan, with the designer owning visual edits.

**Tech Stack:** Rust (serde 1.0.229, uuid 1.27.0, cpal 0.18.2; Tauri 2.12, host-core path dep); Node/React 19.3.0 + TypeScript 7.0.2 + Vite 8.3.3 + Vitest 5.0.3. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-12-channel-pan-design.md` (committed `55bb3d7`, user-approved). This plan implements it and does not alter it.

## Global Constraints

- Mono pan law: `p ∈ [-1,1]`, `L(p)=1-max(p,0)`, `R(p)=1+min(p,0)`, applied **after** gain/mute; `p=0` is `[1,1]` (**no boost**, no compensation).
- Stereo-linked pair: two members with **independent gain and mute**, **one shared pan** as balance, **no crossfeed**; `outL=inputL*gainL*L(p)`, `outR=inputR*gainR*R(p)`. Gains/mutes may differ; the two pans must be equal (single atomic pair edit, one shared ramp).
- Pan is a **position ramp** (not dB). `p` climbs by `2.0/240.0` per frame, so a full `-1→+1` travel (span 2) takes exactly **240 frames at 48 kHz**.
- Preserve PCM/ramps/limiter/codec state across ordinary edits that keep the exact `SessionContext`; reset fresh on identity change. `BUS_HEADROOM_DB` and the limiter are **unchanged**.
- `SourceGain.pan` uses `#[serde(default)]` → omitted decodes to `0.0` (center). Finite out-of-range clamps to `[-1,1]`; malformed/`null`/non-finite is **rejected**.
- Legacy absolute patches that omit `pan` resolve to **center**, never the prior value. Protocol stays **v1 and additive**.
- Pairs are reciprocal (opposite sides), unique IDs, valid physical indices, matching roles, no overlap/dangling peer. Orientation is explicit user choice (non-adjacent or higher-index-left valid). Unpaired → `stereo_pair: null`, `stereo_side: null`. A valid pair needs both members authorized+available; any unavailable/unauthorized side is **fail-closed as a unit** (no mono degradation).
- Routing: every `InputChannel` (paired or not) and every **paired** `MasterLr` use the chosen `stereo_side` / balance law. Only an **unpaired** `MasterLr` keeps the legacy `physical_index % 2` parity split. Unpaired legacy `MasterLr` exposes **no pan/balance**; discovery maps physical channels as mono `InputChannel` unless explicitly paired (no hardware-inferred pairs).
- Device binding = stable CPAL device id string (the real `device.id()`) + the **verified actual selected capture layout** (the built stream's channel count), never the discovery max, name/enumeration order, or an `unknown = 2` fallback. Single device-bound profile, no DB, app-local natively-resolved path, atomic durable saves off the audio path.
- No secrets/epochs/nonces/arm/output/listening permission persisted. `v1` migration preserves IDs/mapping/labels, defaults center, revision `0`. Unresolved legacy pairs require explicit L/R or unlink before start.
- Native commands verify the **actual injected caller window**; operator-only via bundled-window capability; no musician HTTP mutations; do not repurpose unsupported label/publish commands.
- `initialMix(catalog)` is **pure/shared**; sources `gain -60`, `muted: true`, `pan: catalog.defaultPan`; master `-60`, muted. New connection seeds **once**; accepted resume never reseeds; defaults-only saves are **not** broadcast to existing phones. Do not auto-create an accepted mix.
- `mix.applied` is an applied indicator, **not** proof of ramp/DSP/audibility. **No worker-ack redesign.** **Designer owns all visual product edits.** **No paid-model/CI/SonarQube scope.** No commits/pushes by executors; the parent owns commit gates.
- Tests below are **proposed**; none are claimed to exist. Do not claim app tests pass. No live-hardware test is claimed: the real-device path stays an `#[ignore]`/manual check.
- Correct Rust commands: `cargo test` accepts exactly **one** filter, so two modules run as two chained commands (`cargo test -p host-core audio::tests && cargo test -p host-core control::tests`).

## Review Focus

Five highest-risk behaviors, each with an owning test and explicit assertions:

1. **Pan wire compatibility/rejection** — Task 1: `omitted_pan_decodes_as_center` (pan == 0.0), `null_pan_is_rejected`, `string_pan_is_rejected`, `nonfinite_pan_is_rejected` (NaN/Infinity → `Err`), `finite_out_of_range_pan_is_clamped` (−2.0 → −1.0, 5.0 → 1.0).
2. **Atomic pair equality + fail-closed routing** — Task 2: `linked_pair_unequal_pan_is_rejected`, `linked_pair_member_absent_is_rejected_half_pair`, `duplicate_source_id_is_rejected`, `stale_catalog_revision_is_rejected`, `unavailable_pair_member_is_fail_closed_together`.
3. **Pan ramp continuity + identity reset** — Task 2: `pan_ramp_reaches_full_travel_in_240_frames` (239 steps + final == 1.0), `pan_edit_within_context_preserves_partial` (no gap, ramp continues), `pan_resets_on_identity_change` (new `audio_epoch` snaps).
4. **Durable device-bound config + transactional rollback** — Task 3: `wrong_device_binding_is_rejected`, `commit_with_stale_revision_is_rejected_and_memory_unchanged`, `failed_commit_retains_committed_revision`, `corrupt_config_is_surfaced_file_bytes_unchanged`, `v1_migrates_to_v2_preserving_ids_defaults_center`.
5. **Actual-caller auth + shared default seeding** — Task 4: `save_source_links_rejects_foreign_window`; Task 5: `initial_mix_matches_for_both_phone_paths`, `defaults_only_save_does_not_mutate_existing_mix`, `nonzero_default_pan_projects_into_initial_mix` (defaultPan 0.35 → source pan 0.35).

## Files and responsibility map

Single owner per file; sequential tasks may reuse files only as declared. Backend (1–4) precedes frontend (5); UI (6–7) follows the Task 5 contract freeze.

| Path | New/Modify | Owner | Responsibility |
| --- | --- | --- | --- |
| `crates/host-core/src/contract.rs` | Modify | 1 | `SourceGain.pan`, `StereoSide`, `stereo_side`, `default_pan`, `SourcePanDefault`, `ConfigV2` |
| `crates/host-core/src/ids.rs` | Modify | 1 | `SettingsRevision` counter newtype (decimal-string serde) |
| `crates/host-core/src/audio/gain.rs` | Modify | 1 | `pan_coefficients(canonical_p)` |
| `crates/host-core/src/control/revisions.rs` | Modify | 1 | `canonicalize_pan(p) -> Result<f32, ControlError>` |
| `crates/host-core/tests/contract_serde.rs` | Modify | 1 | pan/side serde; malformed `null`/string rejects |
| `crates/host-core/tests/poc_flow.rs` | Modify | 1,8 | fixture callsites; cross-integration |
| `crates/host-core/fixtures/{mix.patch,catalog.snapshot,mix.applied}.json` | Modify | 1 | add `pan`/`stereoSide`/`stereoPair`/`defaultPan` keys |
| `crates/host-core/src/{audio/engine.rs,audio/tests.rs,control/actor.rs,control/tests.rs,server/tests.rs,runtime.rs,pipeline.rs,host.rs}` | Modify | 1 | **all struct-literal callsites** gain new fields (compile-critical) |
| `crates/host-core/src/audio/engine/ramp.rs` | Modify | 2 | `PositionRamp` (2.0/240 step) |
| `crates/host-core/src/audio/engine.rs` | Modify | 2 | routing plan, balance law, fail-closed omission |
| `crates/host-core/src/control/actor.rs` | Modify | 2 | atomic pair validation, stale/duplicate/half-pair reject |
| `crates/host-core/src/config/mod.rs` | Modify | 3 | transactional `ConfigStore` open/commit |
| `crates/host-core/src/config/tests.rs` | **New** | 3 | binding/migration/corruption/rollback |
| `crates/host-core/src/capture/service.rs` (+ helper) | Modify | 3 | authoritative verified `(device_id, channels)` |
| `crates/host-core/src/host.rs` | Modify | 3 | catalog projects defaults; mono/side map |
| `apps/desktop/src-tauri/src/{window_guard,bridge,commands,lib}.rs` | Modify | 4 | exact typed commands, actual-window guard, `start_host` binding |
| `apps/desktop/src-tauri/permissions/operator.toml` + `tests/composition.rs` | Modify | 4 | allow commands + capability acceptance |
| `apps/web/src/protocol/index.ts` | Modify | 5 | `pan`, `stereoSide`/`stereoPair`/`defaultPan`, `SourcePanDefault` |
| `apps/web/src/desktop-bridge/{contracts.ts,tauri-bridge.ts}` | Modify | 5 | typed command surface + actual metadata |
| `apps/web/src/musician/initial-mix.ts` | **New** | 5 | pure `initialMix(catalog)` + `normalizeSourceGain` |
| `apps/web/src/transport/ReceiverController.ts` | Modify | 5 | reuse builder, pan in pending compare |
| `apps/web/src/{musician/MusicianRoot.tsx,musician/ReceiverView.tsx}` | Modify | 5,6 | wiring then grouped strips |
| `apps/web/src/ui/{ChannelStrip,GainControl,PanControl}.tsx` | Modify/New | 6 | designer UI |
| `apps/web/src/admin/SourcesView.tsx` | Modify | 7 | pair/default UI |
| `docs/qualification/channel-pan.md` | **New** | 8 | verification record |

## Dependency / ownership graph

```
Task 1 (contract + pan law + compatibility callsites)
  ├─> Task 2 (control + DSP routing/ramp/safety)
  └─> Task 3 (transactional device-bound config/store)
Task 2, Task 3 -> Task 4 (desktop actual-caller IPC)
Task 4 (frozen Rust bridge) -> Task 5 (TS shared initialMix/pan, single owner)
Task 5 (frozen TS contract) -> Task 6 (phone UI: designer), Task 7 (desktop Sources UI: designer)
Tasks 1..7 -> Task 8 (cross-integration gates + documentation)
```

Tasks 1–4 touch `contract.rs`/`host.rs`/`commands.rs` **sequentially only** (never concurrently): Task 1 owns the struct change, Task 2/3 extend behavior without restructuring the literals again, Task 4 adds IPC. Task 2 and Task 3 do not share files.

---

### Task 1: Wire/config compatibility + canonicalization + all callsites

**Files:**
- Modify: `contract.rs`, `ids.rs`, `audio/gain.rs`, `control/revisions.rs`
- Modify (compile-critical literals): `audio/engine.rs`, `audio/tests.rs`, `control/actor.rs`, `control/tests.rs`, `server/tests.rs`, `runtime.rs`, `pipeline.rs`, `host.rs`
- Modify (tests/fixtures): `tests/contract_serde.rs`, `tests/poc_flow.rs`, `fixtures/mix.patch.json`, `fixtures/catalog.snapshot.json`, `fixtures/mix.applied.json`

**Interfaces:**
- Consumes: existing `SourceGain`, `SourceInfo`, `ChannelMapEntry`, `Config`, `validate_gain`, the `counter_newtype!` macro, `ControlError`.
- Produces (frozen):
  - `SourceGain { source_id: SourceId, gain_db: f32, muted: bool, pan: f32 }` — `pan` `#[serde(default)]` (absent → `0.0`); an explicit `null` is a decode **error**, not center.
  - `StereoSide::{Left, Right}` (`#[serde(rename_all = "camelCase")]`).
  - `ChannelMapEntry.stereo_side: Option<StereoSide>`, `SourceInfo.stereo_side: Option<StereoSide>`, `SourceInfo.default_pan: f32` (all serde-default `None`/`0.0`).
  - `SourcePanDefault { source_id: SourceId, pan: f32 }` (`camelCase`) — the `sourceDefaults` wire element. NO tuple-vector.
  - `SettingsRevision(u64)` in `ids.rs` via `counter_newtype!` (decimal-string serde, like `CatalogRevision`).
  - `ConfigV2 { version: u32, device_id: Option<String>, selected_interface: Option<String>, input_channels: Option<u16>, settings_revision: SettingsRevision, channel_map: Vec<ChannelMapEntry>, labels: Vec<(SourceId, String)>, source_defaults: Vec<SourcePanDefault> }`. The existing `Config` (v1) is **kept unchanged** as the legacy decode target; `ConfigV2` is the new current shape, and migration maps between them explicitly.
  - `pub fn canonicalize_pan(p: f32) -> Result<f32, ControlError>` in `control/revisions.rs`: non-finite → `Err(ControlError::Internal)`; finite clamped to `[-1.0, 1.0]`.
  - `pub fn pan_coefficients(canonical_p: f32) -> [f32; 2]` in `gain.rs` = `[1.0 - canonical_p.max(0.0), 1.0 + canonical_p.min(0.0)]`; **consumes** a canonical value and does **not** validate.

- [ ] **Step 1: Write failing tests** — extend `contract_serde.rs`:

```rust
#[test] fn omitted_pan_decodes_as_center() {
    let g: SourceGain = serde_json::from_str(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false}"#).unwrap();
    assert_eq!(g.pan, 0.0);
}
#[test] fn null_pan_is_rejected() {
    assert!(serde_json::from_str::<SourceGain>(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false,"pan":null}"#).is_err());
}
#[test] fn string_pan_is_rejected() {
    assert!(serde_json::from_str::<SourceGain>(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","gainDb":-6.0,"muted":false,"pan":"left"}"#).is_err());
}
#[test] fn stereo_side_and_default_pan_round_trip() {
    let s: SourceInfo = serde_json::from_str(
        r#"{"sourceId":"33333333-3333-4333-8333-333333333333","physicalIndex":0,"label":"C1","role":"inputChannel","authorized":true,"available":true,"stereoPair":null,"stereoSide":"left","defaultPan":0.35}"#).unwrap();
    assert_eq!(s.stereo_side, Some(StereoSide::Left));
    assert_eq!(s.default_pan, 0.35);
}
```
In `control/revisions.rs` tests (or `control/tests.rs`): `canonicalize_pan(f32::NAN)` and `f32::INFINITY` → `Err`; `-2.0 → -1.0`; `5.0 → 1.0`; `0.35 → 0.35`.

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p host-core --test contract_serde`. Expected: FAIL to compile — `SourceGain` has no `pan`.
- [ ] **Step 3: Implement contract + canonicalization, then fix every callsite.** Add fields/types/validator. Then update **all** struct literals so the crate compiles: `contract.rs` (`find_gain` fallback literal lives in `engine.rs`), `engine.rs`, `audio/tests.rs`, `control/actor.rs` (`canonical.push(SourceGain { .. })` gains `pan: canonicalize_pan(entry.pan)?`), `control/tests.rs`, `server/tests.rs`, `runtime.rs`, `pipeline.rs`, `host.rs`, `tests/poc_flow.rs`, `tests/contract_serde.rs`. Add `pan` to the three fixture JSON payloads and the `stereoSide`/`stereoPair`/`defaultPan` keys where the fixture carries catalog/source data. Do not implement the engine pan math yet (Task 2).
- [ ] **Step 4: Run to verify it passes** — Run: `cargo test -p host-core --test contract_serde && cargo test -p host-core --test poc_flow`. Expected: PASS; crate compiles with all literals updated.
- [ ] **Step 5: Commit gate** — parent stages the contract/ids/gain/revisions files, all callsite files listed above, the two test files, and the three fixtures.

### Task 2: Control + DSP — routing plan, balance law, 240-frame ramp, fail-closed

**Files:**
- Modify: `audio/engine/ramp.rs`, `audio/engine.rs`, `control/actor.rs`
- Modify (tests): `audio/tests.rs`, `control/tests.rs`

**Interfaces:**
- Consumes: Task 1 `SourceGain.pan`, `canonicalize_pan`, `pan_coefficients`, `stereo_side`.
- Produces:
  - `pub struct PositionRamp { current: f32, target: f32 }` with `new(f32)`, `set_target(f32)`, `snap()`, `next() -> f32`; `pub const PAN_STEP_PER_FRAME: f32 = 2.0 / PAN_RAMP_FRAMES_PER_FULL_TRAVEL;` and `pub const PAN_RAMP_FRAMES_PER_FULL_TRAVEL: f32 = 240.0;` (so −1→+1 traverses 240 frames).
  - `pub struct RoutingPlan` (bounded, built once in `AudioEngine::new`): one entry per mapped source carrying `physical_index`, a group id (pair shares one group), `stereo_side`/role, and the `stereo_pair` peer. **Unpaired `MasterLr`** keeps `physical_index % 2` parity; every other entry (all `InputChannel`, paired `MasterLr`) uses the side/balance path.
  - `AudioEngine` holds one `PositionRamp` per **group**; a linked pair advances one shared ramp. Missing/unmapped source contributes **silence** (replacing the unmuted-unity fallback).
  - `ControlActor::apply_patch` rejects: unequal `pan` within a linked pair; a member present without its reciprocal peer (half pair); duplicate source IDs; stale `catalog_revision`; and requires both members authorized+available together.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn pan_ramp_reaches_full_travel_in_240_frames() {
    let mut r = PositionRamp::new(-1.0);
    r.set_target(1.0);
    for _ in 0..239 { r.next(); }
    assert!((r.next() - 1.0).abs() < 1e-6);
}
// pan_edit_within_context_preserves_partial: hold a 100-frame partial, then 20 contiguous frames,
//   same context + changed pan -> out[0].count == 1 (no snap).
// pan_resets_on_identity_change: set target on epoch A; switch to epoch B -> ramp.current == target.
// paired_master_lr_uses_balance_law_not_parity: two linked MasterLr, pan -1.0 -> L == sum(L gain), R silence.
// unpaired_master_lr_keeps_parity_split: one MasterLr index 0 -> left only (legacy index%2).
// linked_pair_sides_reach_distinct_outputs_without_crossfeed: independent L/R sines, gainL != gainR
//   -> left has no R component and vice versa.
// omitted_engine_source_is_silent_not_unity: mapped source absent -> contribution 0.0.
// linked_pair_unequal_pan_is_rejected: pans 0.5/-0.5 -> Err.
// linked_pair_member_absent_is_rejected_half_pair: one member only -> Err.
// duplicate_source_id_is_rejected: two entries, same SourceId -> Err.
// stale_catalog_revision_is_rejected: patch.catalog_revision != actor catalog -> Err(RevisionConflict).
// unavailable_pair_member_is_fail_closed_together: one side available:false -> Err, no mono fallback.
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p host-core audio::tests && cargo test -p host-core control::tests`. Expected: FAIL to compile (`PositionRamp` missing).
- [ ] **Step 3: Implement minimally.** Add `PositionRamp` (stepping by `PAN_STEP_PER_FRAME`); build `RoutingPlan` once; in the per-frame loop use `pan_coefficients(ramp.next())` for non-parity entries and parity only for unpaired `MasterLr`; make a missing mapped source contribute silence; keep preserve-state keyed on exact `SessionContext` and snap ramps on identity change. In `actor.rs`, add pair-equality/half-pair/duplicate/stale-revision/joint-availability validation **before** building the canonical snapshot. No worker-ack changes.
- [ ] **Step 4: Run to verify it passes** — Run: `cargo test -p host-core audio::tests && cargo test -p host-core control::tests`. Expected: PASS; adapted `measure_mono_source_has_no_pan_law_attenuation` asserts center law; limiter/headroom tests unchanged.
- [ ] **Step 5: Commit gate** — parent stages only the five files above.

### Task 3: Transactional device-bound config store + authoritative capture identity

Execution clarification: pre-install I/O failures retain the old file, memory, and revision.
An installed replacement whose directory durability cannot be confirmed returns
`ConfigError::DurabilityUncertain { revision, source }` after reconciling memory to the installed
revision. This distinction overrides blanket unchanged-state wording below. Add deterministic
failure-stage tests. Current-version loads validate complete routing/binding/pair/default state;
sparse load defaults may normalize to center before validation, but saves stay complete. Preserve
compatible migrated IDs and unresolved links during non-destructive layout verification, then
require explicit side resolution/unlinking before persisting a canonical v2 configuration.

**Files:**
- Modify: `config/mod.rs`, `capture/service.rs` (+ a small capture-identity helper), `host.rs`
- Create (tests): `config/tests.rs`

**Interfaces:**
- Consumes: Task 1 `ConfigV2`, `SourcePanDefault`, `SettingsRevision`; existing `ConfigStore`, `catalog_from_channel_map`, `default_channel_map`, `CaptureCapabilities`.
- Produces:
  - `pub enum ConfigError { Io(std::io::Error), Corrupt, Unsupported(u32), RevisionConflict { expected: u64, found: u64 }, DeviceMismatch { bound: String, requested: String } }` (`Io` sources its error; others are explicit).
  - Transactional store:
    ```
    pub struct ConfigStore { path: PathBuf, committed: ConfigV2, revision: u64 }
    impl ConfigStore {
        pub fn open(path: &Path) -> Result<Self, ConfigError>;              // load+migrate+validate; corrupt leaves file bytes untouched
        pub fn binding(&self) -> Option<&str>;                              // bound CPAL device id
        pub fn committed(&self) -> &ConfigV2;
        pub fn commit(&mut self, expected_revision: u64, candidate: ConfigV2) -> Result<ConfigV2, ConfigError>;
        pub fn bind(&mut self, device_id: &str, verified_channels: u16, expected_revision: u64) -> Result<ConfigV2, ConfigError>;
    }
    ```
    `commit` checks `expected_revision` and (when already bound) the candidate's `device_id`, writes `candidate` atomically (temp file + rename) and durably, and replaces the in-memory committed value **only after success**; on `Io` failure the in-memory state and revision are unchanged.
   - `capture::verified_capture_identity(device_id: &str, sample_rate_hz: u32) -> Result<(String, u16), CaptureFault>` — reuses capture startup's stream-configuration selection and returns the real device UID and selected channel count, not the discovery maximum. Setup must not open a second live stream just to read settings; startup verifies that the built stream matches the binding.
  - `default_channel_map` emits mono `InputChannel` (`stereo_side: None`) unless explicit pairs are supplied.

`bind` is the explicit stopped-host rebinding operation: a different device/layout creates fresh source IDs and centered defaults, rather than inheriting the previous map. This is distinct from accidentally changing binding through `commit`.

- [ ] **Step 1: Write failing tests** (`config/tests.rs`, temp dirs)

```rust
// Each test creates a unique temp dir; the store path lives under it.
// wrong_device_binding_is_rejected: open bound to A; bind/commit candidate B -> Err(DeviceMismatch),
//   committed().device_id stays "A".
// commit_with_stale_revision_is_rejected_and_memory_unchanged: expected 0 when current is 1 ->
//   Err(RevisionConflict); committed() unchanged.
// failed_commit_retains_committed_revision: unwritable path -> Err(Io); revision()/committed() unchanged.
// corrupt_config_is_surfaced_file_bytes_unchanged: garbage bytes -> Err(Corrupt); file bytes unchanged.
// v1_migrates_to_v2_preserving_ids_defaults_center: v1 JSON -> device/labels/channel_map preserved;
//   source_defaults all pan 0.0; settings_revision == 0.
// verified_capture_identity_uses_built_stream_channels: fake backend channels == 4 -> ("<id>", 4).
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p host-core config::tests`. Expected: FAIL to compile — new types/functions do not exist.
- [ ] **Step 3: Implement minimally.** Add the transactional store and `ConfigError`; implement atomic write via temp-file-then-rename; never overwrite a corrupt file; add `verified_capture_identity` in the capture module. In `host.rs`, project committed defaults into `catalog_from_channel_map` and keep `default_channel_map` mono/side; a defaults-only save must **not** advance the map revision, and a fresh connection reads store defaults with no broadcast.
- [ ] **Step 4: Run to verify it passes** — Run: `cargo test -p host-core config::tests && cargo test -p host-core host::tests`. Expected: PASS; `default_channel_map_is_centered_input_channels` still passes.
- [ ] **Step 5: Commit gate** — parent stages `config/mod.rs`, `config/tests.rs`, `capture/service.rs`, `host.rs`.

### Task 4: Desktop actual-caller IPC — exact typed commands

Consume the store's distinct durability-uncertain outcome: report that installation occurred but
restart durability is unconfirmed, reload current settings/revision, and never retry using a stale
base or label the operation ordinary success. Confirm preflight device binding against the actual
started capture layout through the host/runtime accessor. Fresh-connection catalog projection is a
composition requirement; default saves must not broadcast to existing musicians.

**Files:**
- Modify: `window_guard.rs`, `bridge.rs`, `commands.rs`, `lib.rs`, `permissions/operator.toml`, `tests/composition.rs`

**Interfaces:**
- Consumes: Task 2/3 store, validation, and `capture::verified_capture_identity`; existing `authorize_window`.
- Produces:
  - `start_host` composition change: `commands.rs::start_host` resolves channels through `capture::verified_capture_identity` (the built stream's layout) rather than the discovery maximum, and binds the store to that `(device_id, channels)` at stopped-host setup.
  - `window_guard::authorize_call(window: &tauri::Window) -> Result<(), WindowDenied>` — reads the **actual injected window's** label; `authorize_window(&str)` stays for unit tests.
  - Bridge DTOs (exact): `SourceInfo { source_id: String, physical_index: u16, label: String, role: String, available: bool, available_to_musicians: bool, stereo_pair: Option<String>, stereo_side: Option<String>, default_pan: f32 }`; `SourceSettings { device_id: String, input_channels: u16, settings_revision: String, catalog: CatalogSnapshot }`; `SourcePanDefaultDto { source_id: String, pan: f32 }`; `SourceLinkPair { left_source_id: String, right_source_id: String }`.
  - Commands (exact typed signatures; `device_id` is a **required** `String` because these operate on the selected device):
    ```rust
    #[tauri::command]
    pub fn read_source_settings(window: tauri::Window, state: State<'_, HostState>, device_id: String)
        -> Result<SourceSettings, IpcError>;
    #[tauri::command]
    pub fn save_source_defaults(window: tauri::Window, state: State<'_, HostState>, device_id: String,
        base_settings_revision: String, source_defaults: Vec<SourcePanDefaultDto>)
        -> Result<SourceSettings, IpcError>;
    #[tauri::command]
    pub fn save_source_links(window: tauri::Window, state: State<'_, HostState>, device_id: String,
        base_settings_revision: String, pairs: Vec<SourceLinkPair>)
        -> Result<SourceSettings, IpcError>;   // requires STOPPED host; resets affected defaults to center
    ```
    Each has a testable core `*_impl` taking `(window_label: &str, state: &HostState, device_id: &str, base_revision: &str, payload)`.

- [ ] **Step 1: Write failing tests** (inline `commands.rs` + `composition.rs`)

```rust
#[test] fn save_source_links_rejects_foreign_window() {
    let state = HostState::default();
    assert_eq!(save_source_links_impl("musician", &state, "dev".into(), "0".into(), vec![])
        .unwrap_err().code, "WINDOW_FORBIDDEN");
}
#[test] fn read_source_settings_rejects_foreign_window() {
    let state = HostState::default();
    assert_eq!(read_source_settings_impl("other", &state, "dev".into()).unwrap_err().code, "WINDOW_FORBIDDEN");
}
// fn save_source_links_requires_stopped_host:
//   state with a running host -> Err with a typed code; assert no commit happened.
// fn save_source_defaults_validates_revision_and_binding:
//   stale base revision -> Err(RevisionConflict); a mismatched bound device -> Err(DeviceMismatch).
// fn start_host_binds_verified_capture_layout: start_host resolves channels via
//   verified_capture_identity (built stream), not the enumeration max; store binding matches.
```
In `composition.rs`, assert the operator capability allows the three commands and a non-operator window is denied.

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p iem-cast-desktop`. Expected: FAIL to compile — commands/DTO fields missing.
- [ ] **Step 3: Implement minimally.** Add the actual-window guard and the three commands with `_impl` cores; register in `lib.rs` and `permissions/operator.toml` (exact command names); extend `bridge.rs` DTOs. Validate revision/binding/pair equality server-side. Do not wire defaults-only saves to any broadcast path.
- [ ] **Step 4: Run to verify it passes** — Run: `cargo test -p iem-cast-desktop`. Expected: PASS; existing `WINDOW_FORBIDDEN`/monitor tests unchanged.
- [ ] **Step 5: Commit gate** — parent stages the six desktop files.

### Task 5: TS shared `initialMix`, legacy pan normalization, protocol/bridge — single owner

**Files:**
- Modify: `protocol/index.ts`, `desktop-bridge/contracts.ts`, `desktop-bridge/tauri-bridge.ts`, `transport/ReceiverController.ts`, `musician/MusicianRoot.tsx`, `musician/ReceiverView.tsx`
- Create: `musician/initial-mix.ts`
- Modify (tests): `musician/__tests__/MusicianRoot.test.tsx`, `musician/__tests__/mix.browserHarness.tsx`, `admin/__tests__/browserHarness.tsx`, `admin/__tests__/SourcesView.test.tsx`
- Create (tests): `musician/__tests__/initial-mix.test.ts`

**Interfaces:**
- Consumes: Task 1/4 wire shapes.
- Produces:
  - Types: `SourceGain { sourceId; gainDb; muted; pan: number }`; incoming legacy snapshots may omit `pan`; `SourcePanDefault { sourceId: string; pan: number }`; `SourceInfo` gains `stereoSide: 'left' | 'right' | null; defaultPan: number`.
  - `export function normalizeSourceGain(g: { sourceId: string; gainDb: number; muted: boolean; pan?: number }): SourceGain` — fills omitted `pan` with `0` (center); used on every incoming snapshot/ack so the rest of the app can treat `pan` as always present without regressing legacy payloads.
  - `export function initialMix(catalog: CatalogSnapshot): MixSnapshot` (pure; sources `gainDb -60`, `muted true`, `pan: source.defaultPan`; master `-60`, muted; revision `'0'`).
  - `HostBridge` typed methods: `readSourceSettings(deviceId: string): Promise<SourceSettings>`, `saveSourceDefaults(deviceId: string, baseSettingsRevision: string, sourceDefaults: SourcePanDefault[]): Promise<SourceSettings>`, `saveSourceLinks(deviceId: string, baseSettingsRevision: string, pairs: SourceLinkPair[]): Promise<SourceSettings>`.
  - `ReceiverController` routes its neutral-seed site through `initialMix` and includes `pan` in the pending/ack compare.
  - `MusicianRoot` passes `onChannelPanChange(sourceId, pan)` and pair-balance callback props to `ReceiverView`; the designer wires visuals later (Task 6), which may mechanically edit these props only.

- [ ] **Step 1: Write failing tests** — no comment-only tests:

```ts
// initial-mix.test.ts — `catalogWith(defaultPan)` builds the one-source CatalogSnapshot.
it('initial_mix_matches_for_both_phone_paths', () => {
  const c = catalogWith(0);
   const a = initialMix(c);
  expect(a.sources).toEqual([{ sourceId: 'a', gainDb: -60, muted: true, pan: 0 }]);
  expect(a.masterDb).toBe(-60); expect(a.masterMuted).toBe(true);
});
it('nonzero_default_pan_projects_into_initial_mix', () => {
  expect(initialMix(catalogWith(0.35)).sources[0].pan).toBe(0.35);
});
it('legacy_snapshot_without_pan_normalizes_to_center', () => {
  expect(normalizeSourceGain({ sourceId: 'a', gainDb: -3, muted: false }).pan).toBe(0);
});
```
In controller and musician tests, compare actual arm-bootstrap and first-edit patches with the shared template, accounting for the explicit edit. Use a nonzero default to catch either path ignoring defaults. Test rapid pan edits, accepted/applied lag, rejection rollback, and reconnect retaining accepted pan rather than reseeding. Update the mock metadata/settings fields in browser harnesses.

- [ ] **Step 2: Run to verify it fails** — Run: `npm test --workspace apps/web`. Expected: FAIL — `initialMix` missing / type errors from new fields.
- [ ] **Step 3: Implement minimally.** Add the types, `normalizeSourceGain`, and the pure builder; route both `MusicianRoot` and `ReceiverController` through it; extend the pending-compare to include `pan`; add the three bridge methods; update all TS mock literals. Do not restyle UI.
- [ ] **Step 4: Run to verify it passes** — Run: `npm test --workspace apps/web && npm run typecheck --workspace apps/web`. Expected: PASS.
- [ ] **Step 5: Commit gate** — parent stages the six modified web files and the new test files. After this task the shared protocol/bridge is frozen; Tasks 6–7 must not edit `protocol/index.ts` or `desktop-bridge/*`.

### Task 6: Phone UI — grouped pair strips, mono pan (designer)

**Files:**
- Modify: `ui/ChannelStrip.tsx`, `ui/GainControl.tsx`, `musician/ReceiverView.tsx`, `musician/MusicianRoot.tsx`
- Create: `ui/PanControl.tsx`, `ui/__tests__/PanControl.test.tsx`

**Interfaces:**
- Consumes: Task 5 `SourceInfo.stereoSide/defaultPan`, `SourceGain.pan`, `onChannelPanChange`, pair-balance callback.
- Produces (designer-owned):
   - `PanControlProps { label: string; value: number; onChange(value: number): void; onReset(): void; disabled?: boolean; fullLabel?: string }`; props and callbacks use normalized `[-1,1]`, while the native range displays `-100..100`, step `1`. Multiply/divide by 100 at this boundary only. Readout `Center`/`25% left`/`full right`; Center reset; accessible name with percentage; ≥48 px target; no mute side effect.
  - `ReceiverView` renders a linked pair as **two grouped `ChannelStrip`s with one shared balance `PanControl` below**; mono shows pan immediately after gain.
  - `ReceiverView` callback props consumed from `MusicianRoot`: `onChannelGainChange`, `onChannelMuteChange`, `onChannelPanChange(sourceId, pan)`, and a pair-level `onPairBalanceChange(leftSourceId, rightSourceId, pan)`.

- [ ] **Step 1: Write failing test** (`ui/__tests__/PanControl.test.tsx`)

```tsx
it('pan_control_renders_center_and_resets', () => {
  const onChange = vi.fn(); render(<PanControl label="A" value={0.5} onChange={onChange} onReset={() => onChange(0)} />);
  expect(screen.getByRole('slider').getAttribute('aria-valuetext')).toMatch(/right/i);
  fireEvent.change(screen.getByRole('slider'), { target: { value: '-50' } });
  expect(onChange).toHaveBeenCalledWith(-0.5);
  fireEvent.click(screen.getByRole('button', { name: /center/i }));
  expect(onChange).toHaveBeenLastCalledWith(0);
});
it('pan_control_disabled_does_not_fire_mute', () => {
  const onChange = vi.fn(); render(<PanControl label="A" value={0} onChange={onChange} onReset={() => {}} disabled />);
  expect((screen.getByRole('slider') as HTMLInputElement).disabled).toBe(true);
  fireEvent.change(screen.getByRole('slider'), { target: { value: '10' } });
  expect(onChange).not.toHaveBeenCalled();
});
// linked_pair_renders_two_gain_controls_one_balance:
//   within the linked-pair group, query role=slider -> exactly 3 (2 gain + 1 balance).
// unavailable_pair_disables_its_controls:
//   pair member available:false; assert all 3 sliders are disabled.
```

- [ ] **Step 2: Run to verify it fails** — Run: `npm test --workspace apps/web`. Expected: FAIL — `PanControl` missing.
- [ ] **Step 3: Implement (designer)** the control and layout per spec §5.1 using existing tokens/classes; no new design system.
- [ ] **Step 4: Run to verify it passes** — Run: `npm test --workspace apps/web && npm run build:web`. Expected: PASS and build succeeds.
- [ ] **Step 5: Commit gate** — parent stages only the UI files this task touched.

### Task 7: Desktop Sources pair/default UI (designer)

**Files:**
- Modify: `admin/SourcesView.tsx`
- Create: `admin/__tests__/SourcesPan.test.tsx`

**Interfaces:**
- Consumes: Task 4/5 command surface and `SourceInfo` metadata.
- Produces: heading **"Defaults for new personal mixes"**, controls labelled **"Default pan"**/**"Default balance"**; link/unlink/reverse enabled only when host stopped; `Saving`/`Saved`/`failed · Retry`; stale-response isolation by device binding/revision; disabled truthful controls while loading/unreadable.

- [ ] **Step 1: Write failing test** (`SourcesPan.test.tsx`, mock `HostBridge`)

```tsx
// Uses SourcesView.test.tsx's `bridgeWith(byDevice)` helper extended to host the new methods.
// link_actions_disabled_while_host_running: running host -> link button disabled === true.
// default_pan_change_calls_save_source_defaults: slider to -50; commit -> saveSourceDefaults(
//   deviceId, currentRevision, completeDefaultsWithChangedSourcePan(-0.5)).
// save_failure_shows_retry_and_stale_response_is_ignored: first save rejects -> "failed" + Retry;
//   a later resolve with an older settingsRevision does not overwrite the draft.
// device_binding_change_isolates_in_flight_save: switch device mid-flight -> old response discarded.
```

- [ ] **Step 2: Run to verify it fails** — Run: `npm test --workspace apps/web`. Expected: FAIL — new controls missing.
- [ ] **Step 3: Implement (designer)** the pair/default UI using the frozen bridge.
- [ ] **Step 4: Run to verify it passes** — Run: `npm test --workspace apps/web && npm run build:web`. Expected: PASS.
- [ ] **Step 5: Commit gate** — parent stages `SourcesView.tsx` and its test.

### Task 8: Cross-integration gates and documentation

**Files:**
- Modify: `crates/host-core/tests/poc_flow.rs`
- Create: `docs/qualification/channel-pan.md`
- Read-only evidence: `apps/web/src/musician/__tests__/mix.browserHarness.tsx`, `apps/web/e2e/poc.spec.ts`

**Interfaces:**
- Consumes: Tasks 1–7.
- Produces: a verification record and cross-cutting acceptance coverage (legacy wire, queue/ack pan compare, defaults A/B, saved-restart no playback restore, auth spoof, 320 px).

- [ ] **Step 1: Write/complete the cross-integration test** (in `poc_flow.rs`, using existing `source`/`session_a`/`ctx`/`patch`/`actor`/`nonce` helpers):

```rust
// Task 1 extends this file's `patch` to take pan: `patch(gain_db: f32, pan: f32)`.
#[test] fn defaults_only_save_leaves_old_mix_unchanged_and_new_mix_takes_b() {
    let mut a = actor();
    assert!(a.apply_patch(session_a(), patch(-3.0, -0.4)).is_ok());
    let accepted = a.apply_patch(session_a(), patch(-3.0, 0.4)).unwrap();
    // Commit defaults B through the real store before the assertion; never modify the actor mix.
    assert_eq!(a.accepted_snapshot(session_a()).unwrap().sources.as_slice()[0].pan, 0.4);
    assert_ne!(accepted.accepted_revision, MixRevision(0));
    // Verify a fresh catalog projects B, and the TS initialMix integration produces B muted.
}
```
The final bodies use the real store actor wiring; the assertions above are the minimum required shape (accepted pan stability + fresh-template pan).

- [ ] **Step 2: Run to verify it fails** — Run: `npm run build:web && cargo test -p host-core --test poc_flow`. Expected: FAIL on the new assertion until wired.
- [ ] **Step 3: Complete the test and write `docs/qualification/channel-pan.md`.** Extend only `poc_flow.rs` (no wiring in files owned by earlier tasks). For the browser/restart evidence, the doc records the actual fixtures used: `apps/web/src/musician/__tests__/mix.browserHarness.tsx` (live controller) and the `SourcesView.test.tsx` browser harness — plus the existing `apps/web/e2e/poc.spec.ts` layout tests for 320 px. Record the manual hardware check separately: host audio output muted or at a safe low volume; feed a stereo signal and confirm distinct L/R; **state clearly that no live physical test was run** unless one actually was.
- [ ] **Step 4: Run the full gate** — Run, in order:
   `npm test --workspace apps/web && npm run build:web && cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.
  Expected: PASS. Record actual results; do not claim tests ran if they did not.
- [ ] **Step 5: Commit gate** — parent stages `poc_flow.rs` and `docs/qualification/channel-pan.md`.

---

## Notes

- `cargo test` takes a single filter; per-module runs are chained commands. The workspace build order is web first (`npm run build:web`), then `cargo test -p host-core`, then `cargo test -p iem-cast-desktop`, because the Tauri crate embeds frontend assets.
- Tasks 6–7 are the only visual tasks; all other lanes must not style UI. Do not write implementation until the user reviews this plan and selects an execution method.
