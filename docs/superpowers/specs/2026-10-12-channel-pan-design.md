# IEM Cast — Channel Pan / Stereo Balance Design

Status: scope approved; written specification awaiting user review. This document is architectural
intent for implementers; it is not an implementation plan and makes no claim that described code or
tests currently exist.

## 1. Scope and non-goals

### Scope

- Add a per-source **pan/balance** position to every source's personal mix (`SourceGain.pan`).
- Mono sources get a pan law; stereo-linked pairs get a balance law with two independent
  gain/mute members and one shared balance.
- Let the desktop operator define, link, unlink, and reverse stereo pairs only while the host
  is stopped. Default pan/balance edits are allowed while running and affect only new mixes.
- Add operator IPC read/save endpoints for source settings, links, and defaults, plus versioned
  persistence (`ConfigV2`) bound to a verified capture device identity.
- Let phones render grouped stereo strips (independent gain/mute, one balance) and mono pan
  controls, using one shared default-mix builder on both phone paths.

### Non-goals

- No master pan; the personal master stays attenuation + mute only.
- No host-side audio-loop pairing, gain-staging, or I/O; a stereo link is a **routing/plan**
  construct, not a realtime pairing/I/O operation.
- No change to the −6 dB fixed bus headroom, the stereo-linked limiter, Opus/encoder, or the
  safety gate.
- No redesigned worker acknowledgement, no new mixer-return or crossfeed signal path, and no
  repurposing of the unsupported source label/publish commands.
- No broad refactor of unrelated subsystems.

## 2. Current seams (inspection summary)

The following existing behavior is what this design extends. Names and line ranges are taken from
the tree as inspected.

- `crates/host-core/src/contract.rs`: `SourceGain {source_id, gain_db, muted}` (no pan);
  `ChannelMapEntry {physical_index, source_id, stereo_pair, role}`; `SourceInfo {…, stereo_pair}`;
  `SourceRole::{InputChannel, MasterLr}`; `Config` at `version = 1`. Wire DTOs camelCase; counters
  are decimal strings.
- `crates/host-core/src/audio/gain.rs`: `gain_linear`, `validate_gain` (non-finite rejected, finite
  clamped to `[-60, 0]`), `centered_coefficients() == [1.0, 1.0]` (mono dual-mono, no pan).
- `crates/host-core/src/audio/engine.rs`: per-frame sum with per-channel `Ramp`; `InputChannel`
  applies centered coefficients, `MasterLr` splits by `physical_index % 2`. Ramps preserve state on
  equal `SessionContext`, snap on identity change.
- `crates/host-core/src/pipeline.rs`: `register` preserves engine/encoder/safety only on exact live
  session + context match.
- `crates/host-core/src/control/actor.rs`: `apply_patch` validates catalog authorization,
  canonicalizes gain, rejects stale `base_revision`, forbids unarmed unmute.
- `crates/host-core/src/host.rs`: `catalog_from_channel_map` builds `SourceInfo`;
  `default_channel_map` makes every physical channel an `InputChannel` with `stereo_pair: None`.
- `crates/host-core/src/config/mod.rs`: in-memory `ConfigStore` at `CONFIG_VERSION = 1`.
- Desktop: `commands.rs` (`set_available_sources`/`set_source_label` return
  `SOURCE_EDIT_UNSUPPORTED`), `bridge.rs` (`SourceInfo` lacks role/pair), `lib.rs`,
  `permissions/operator.toml`.
- Web: `protocol/index.ts`, `desktop-bridge/contracts.ts`, `desktop-bridge/tauri-bridge.ts`
  (`toSource` hardcodes `role: 'inputChannel'`, `stereoPair: null`), `musician/MusicianRoot.tsx`
  (neutral mix lines 75–89), `transport/ReceiverController.ts` (neutral mix lines 485–502),
  `musician/ReceiverView.tsx`, `ui/ChannelStrip.tsx`, `ui/GainControl.tsx` (attenuation-only),
  `admin/SourcesView.tsx`.

## 3. Wire and config compatibility

### 3.1 Pan on the wire

- `SourceGain` gains `pan: f32`. Native: `#[serde(default)]` so an omitted pan decodes as `0.0`
  (**center**); once present it is required, explicit state with no "previous value" fallback.
- Finite values outside `[-1.0, 1.0]` are clamped and canonicalized, consistent with the gain rule.
  Malformed, `null`, or non-finite pan values are **rejected**, never clamped-to-success.
- Legacy absolute patches that omit `pan` resolve to **center**, not the source's prior value, so
  old clients stay deterministic.
- `SourceInfo` gains `default_pan` (numeric; absent legacy input decodes as `0.0`). Metadata DTOs
  that now carry floats drop `Eq` where needed and keep `PartialEq`. Protocol stays **v1 and
  additive**: no envelope, type, or counter-format change.

### 3.2 Stereo side and pairing metadata

- `ChannelMapEntry` and `SourceInfo` gain `stereo_side: Option<StereoSide>` where
  `StereoSide = 'left' | 'right'`, alongside the existing `stereo_pair: Option<SourceId>`.
- An unpaired source has `stereo_pair: null` and `stereo_side: null`.
- Pairs are **reciprocal**: each member's `stereo_pair` is the other member's `SourceId`, and the
  two members carry opposite `stereo_side` values. Both members must have unique source IDs and
   valid physical indices, matching roles (both `InputChannel` or both `MasterLr`), no overlap with any other pair, and no
  dangling peer. Orientation is **explicit and user-chosen**: a non-adjacent pair, or a pair whose
  left member has the higher physical index, is valid because the operator chose it.
- A "valid pair" requires both members authorized and available. If either side is unavailable or
  unauthorized the pair is **fail-closed**: it is treated as a unit and is not silently degraded
  into a mono source.
- Unpaired legacy `MasterLr` sources keep their existing parity-split behavior unchanged and expose
  **no pan/balance control**.
- Discovery maps each physical channel as a **mono `InputChannel`** unless the operator explicitly
  pairs it; no pair is inferred from hardware.

## 4. DSP safety

### 4.1 Laws

Define pan `p ∈ [-1.0, 1.0]`. For a mono `InputChannel`, pan is applied **after** its gain/mute:

```
L(p) = 1 - max(p, 0)
R(p) = 1 + min(p, 0)
outL = sample * gain * L(p)
outR = sample * gain * R(p)
```

At `p = 0` this is `[1, 1]` — the existing dual-mono center level with **no boost** and no pan-law
gain compensation. `p = -1` yields full left, `p = +1` full right.

For an explicitly stereo-linked pair, the two members have independent gain and mute but share
**one** pan interpreted as balance, with **no crossfeed**:

```
outL = inputL * gainL * L(p)
outR = inputR * gainR * R(p)
```

Gains and mutes need **not** be equal. Both members must carry the **same pan**, and a patch that
changes them is a single atomic pair edit; unequal pans across a linked pair are rejected.

### 4.2 Ramping and state preservation

- Pan is a position ramp, not a dB ramp, moving over the full `-1 → +1` span in **240 frames at
  48 kHz** (5 ms), the same nominal window as the existing gain ramp.
- Under a stereo link the two members advance from one shared position once per frame.
- Ordinary edits (gain, mute, pan, revision) that keep the **exact** `SessionContext`
  (`session_epoch` + `audio_epoch` + `safety_generation`) preserve partial PCM, gain ramps, pan
  ramps, limiter, and codec state — no sample gap and no ramp discontinuity.
- A changed session/audio/safety identity resets PCM, ramps, pan position, limiter, and codec
  state fresh; stale partial audio is never re-tagged or replayed.
- The −6 dB headroom and the stereo-linked limiter are unchanged; pan and balance operate before
  them, consistent with the existing chain order.

### 4.3 Fail-closed resolution

- When a source is missing from a snapshot, the engine must **fail closed** rather than today's
  fallback of unmuted unity gain. A missing/unmapped source contributes silence.
- Pair completeness is validated at the control boundary (see §8). An incomplete or invalid pair
  never reaches the DSP as a partial link.

## 5. UI

### 5.1 Phone (musician)

- A linked stereo pair renders as **two grouped strips**: independent gain and mute per member,
  with **one balance control below** the pair. There is **no master pan**.
- A mono source places Pan immediately after the gain control, before status and mute actions.
- Balance/pan range is **−100..100, step 1**, mapped to normalized `[-1, 1]`. Readout shows
  `Center` at 0, `25% left`/`full right` style phrases at the extremes. A **Center** reset control
  is provided.
- The control is keyboard-operable and uses a native slider, with an accessible name that
  identifies the source or pair and the percentage/position. Touch target is at least 48 px and the
  layout is responsive down to 320 px.
- Existing visual style is retained; no new design system is introduced.
- Controls for an invalid or unavailable pair are **disabled**. Disabling a control has **no mute
  side effect**.
- The existing `mix.applied` event remains the applied indicator only. It is **not** proof that the
  ramp, DSP output, or audibility is correct; a worker-ack redesign is out of scope.

### 5.2 Desktop (operator)

- Source settings live in **setup step 4**. Operators may persist explicit L/R pair selection,
  link, unlink, and reverse. These write actions are permitted **only while the host is STOPPED**,
  enforced natively.
- Changing routing invalidates stale sessions/map state. A routing change resets affected members'
  **defaults to center only** (it does not reset gains/mutes).
- Per-source **default** pan edits are allowed while the host is running, but must **never** alter
  live personal mixes or routing epochs.
- The desktop heading is **"Defaults for new personal mixes"**, with controls labelled
  **"Default pan"** or **"Default balance"**. Saved defaults never change existing mixes.
- Saves are serialized/coalesced. The UI distinguishes last committed value from an unsaved draft
  and shows `Saving` / `Saved` / `failed · Retry`. Late responses are isolated by device
  binding/revision so a stale reply cannot overwrite newer intent.
- While loading, or when storage is unreadable, controls are disabled and the state is stated
  truthfully. The UI never fabricates a saved center value and never overwrites a corrupt file.

## 6. Persistence and device binding

`ConfigV2` has exactly these fields:

```
version: 2
deviceId: string | null
selectedInterface: string | null
inputChannels: number | null
settingsRevision: CounterString
channelMap: ChannelMapEntry[]
labels: [SourceId, string][]
sourceDefaults: { sourceId, pan }[]
```

- There is a **single device-bound profile**; no database.
- Device identity is a **stable CPAL device UID** plus the **verified actual selected capture
  layout** (resolved from the real stream/device, never from the device name, enumeration order,
  advertised maximum capabilities, or a fallback `unknown = 2`).
- New device bindings generate fresh UUIDs; the channel map's existing source IDs are retained
  across restarts and pair changes.
- A changed device or layout requires explicit **stopped-host rebinding**; defaults are never
  silently reused across a device/layout change.
- The persistence path is a fixed **app-local path resolved natively**. Saves are validated,
  migrated, serialized, atomic, and durable, and happen **off the audio path**.
- A failed save retains the committed config revision. Corrupt or unsupported files are surfaced
  visibly and recovered from; they are never overwritten in place.
- Failure before installing the new file preserves the prior file, memory, and revision. If an
  atomic replacement succeeds but directory durability cannot be confirmed, reconcile memory to
  the installed revision and return a distinct durability-uncertain result. The UI must not report
  ordinary success or claim the original file was unchanged; reload current settings before retry.
- `v1` migration preserves IDs, mapping, and labels; migrated defaults are **center**, revision
  `0`, and the input binding is verified before the save.
- **Unresolved legacy pairs** carry no inferred side: the operator must explicitly set L/R or unlink
  before the host can start.
- No secrets, epochs, nonces, arm/output state, or listening permission are ever persisted.
- A device rename with a stable UID is acceptable. External patch changes to the device that do not
  change the UID cannot be detected.

## 7. Initialization and resume

- A shared **pure** `initialMix(catalog)` is the single default-mix builder and is reused by
  `MusicianRoot.tsx` (lines 75–89) and `ReceiverController.ts` (lines 485–502). The host must **not**
  auto-create an accepted mix.
- The template snapshot uses the catalog revision and mix revision `0`; every usable, authorized
  source is `gain -60`, `muted: true`, `pan: catalog.defaultPan`; master is `-60`, muted.
- Complete valid pairs seed both members consistently from the same template pass, so the two phone
  paths cannot drift.
- The host's initial catalog projects committed defaults from the shared store. Defaults-only saves
  are **not** broadcast to existing phones.
- A requested/accepted/applied mix always takes precedence over the template. A new connection with
  no mix seeds the template **once**; an accepted session resume **never** reseeds.
- Stop retains personal settings locally. Reload leaves local output closed. A host restart restores
  config but starts fresh sessions with no playback restore.

## 8. Operator APIs

### 8.1 Commands

- `read_source_settings({ deviceId }) -> { deviceId, inputChannels, settingsRevision, catalog }`.
- `save_source_defaults({ deviceId, baseSettingsRevision, sourceDefaults })` — takes a **complete**
  vector and validates pair equality, `settingsRevision`, and device binding. It must not install a
  mix or change routing.
- `save_source_links({ deviceId, baseSettingsRevision, pairs: [{ leftSourceId, rightSourceId }] })`
  — replaces the complete pair set atomically. It requires a **stopped host** and resets affected
  defaults to center, advancing the catalog/routing binding.

### 8.2 Authorization and wiring

- Native commands verify the **actual injected caller window**, not a frontend-supplied
  `windowLabel`. Access is operator-only via the **bundled-window capability**; there are **no
  musician HTTP mutations**.
- Register the new commands in `lib.rs` and `permissions/operator.toml`.
- The Rust/TS bridge must carry **actual** role/pair/side/default metadata; the current adapter
  hardcodes `inputChannel`/`null` and must be corrected.
- The unsupported source label/publish commands are **not** repurposed for this work.

### 8.3 Validation

- Validate catalog revision, duplicate IDs, authorization, and pair completeness. Reject anything
  incomplete or inconsistent. Source omissions in the engine fail closed (§4.3).
- Keep the change scoped: no unrelated broad refactor and no worker-ack redesign.

## 9. Acceptance and verification

Acceptance coverage (proposed; none of these tests are claimed to exist yet):

- Legacy wire: omitted-`pan` patches decode as center and old clients keep working.
- Queue/ack pan comparisons distinguish requested, accepted, and applied values truthfully,
  including pending edits, rejection, and rollback.
- Mono pan law with **no boost** at center; `L(p)`/`R(p)` boundaries correct.
- Distinct L/R with no crossfeed and independent mute on a linked pair.
- Smooth continuity across gain/pan edits within one context, and clean reset across context change.
- Invalid links rejected; stale saves rejected; pair changes while running rejected.
- Defaults A/B: an old accepted mix is unchanged, a new mix picks up B.
- Saved-then-restart restores config with no playback restore.
- Wrong device binding and corrupt writes are safe (fail closed, no overwrite).
- Auth spoof (frontend-supplied window label) rejected.
- UI: 320 px phone layout and desktop browser behavior.

Existing source tests that assert gain law and "no pan / dual-mono" must be adapted to linked-only
cases while **preserving mono behavior**; they are not deleted.

Proposed verification commands (to be run once implementation exists): `npm test` and
`npm run build:web`, then `cargo test --workspace --locked` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`, plus
targeted browser receiver + `SourcesView` fixtures.

## 10. Ownership and dependencies

- **Backend contracts first**, then core DSP/config.
- **Desktop** commands + store.
- **Frontend protocol integration** by a single owner.
- **Designer** owns the UI.
- **Parent** reconciles. Do not write the implementation plan yet.

Relevant paths: `crates/host-core/src/{contract.rs,config/mod.rs,audio/gain.rs,audio/engine.rs,
control/actor.rs,host.rs,pipeline.rs}`;
`apps/desktop/src-tauri/src/{commands.rs,bridge.rs,lib.rs}` and
`apps/desktop/src-tauri/permissions/operator.toml`;
`apps/web/src/{protocol/index.ts,desktop-bridge/contracts.ts,desktop-bridge/tauri-bridge.ts,
musician/MusicianRoot.tsx,musician/ReceiverView.tsx,transport/ReceiverController.ts,
ui/ChannelStrip.tsx,ui/GainControl.tsx,admin/SourcesView.tsx}`.

## 11. Limitations

- A pan/balance ramp, `mix.applied`, or the UI `Applied` label is not an audibility or
  hearing-safety guarantee.
- Device identity is only as stable as the CPAL UID plus verified layout; external changes that keep
  the UID cannot be detected.
- The mobile and desktop acceptance criteria above are proposed, not yet verified.
- This document assumes the existing −6 dB headroom and limiter behavior; it does not re-qualify
  them.
