# IEM Cast — macOS POC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a reusable macOS POC that captures real Soundcraft USB multichannel audio, mixes two independent personal monitor feeds, and delivers each as a stereo WebRTC stream to a phone browser through a trusted local HTTPS UI.

**Architecture:** One Rust `host-core` process owns capture, DSP, per-listener Opus encoding, WebRTC transport, control/safety, and the local HTTPS server. A minimal Tauri operator window composes the process and exposes a narrow IPC bridge; React + TypeScript + Tailwind phone and admin views consume frozen protocol types and a frozen receiver controller. Transport is str0m (candidate, not build-validated). Capture targets macOS CoreAudio first with Windows/Linux seams preserved and untested.

**Tech Stack:** Rust (cpal 0.18.2, rtrb 0.4.0, opus 0.4.0 + opusic-sys 0.7.3 bundled, str0m 0.24.1, Tokio 1.53.2, Axum 0.8.9, axum-server 0.8.0 + tls-rustls, serde 1.0.229/serde_json 1.0.151, uuid 1.27.0, getrandom 0.4.3, if-addrs 0.14.0, rust-embed 8.12.0, Tauri 2.12.1 runtime + tauri-build 2.7.1); Node workspaces (React 19.3.0, react-dom 19.3.0, TypeScript 7.0.2, Vite 8.3.3, @vitejs/plugin-react 6.1.2, @tailwindcss/vite 4.3.3, qrcode.react 4.2.0, Vitest 5.0.3, @testing-library/react 16.3.3 + dom 10.4.2 + user-event 14.6.7 + jest-dom 7.0.1, jsdom 30.1.2, @playwright/test 1.63.0, @tauri-apps/cli 2.12.1, @tauri-apps/api 2.12.1).

**Spec:** `docs/ARCHITECTURE.md` (especially §1.1 First reusable POC — macOS and §17.1 Mobile diagnostics contract) and `docs/DESIGN-SYSTEM.md` (§4 Shared component contract, including its `LatencyDiagnostics` subsection, and §5 states). README holds approved product scope. These documents travel with this plan; executors read both.

## Global Constraints

Copied verbatim in effect from the spec; every task implicitly includes this section.

- Capture is **input-only**; **no USB RTN** return to the mixer; source facts and master/analog-send semantics are fixed as in README and ARCHITECTURE §1.
- Capture rate is **48 kHz only when the actual device verifies it**; unsupported rates are **rejected, never silently resampled**.
- Streamed personal mixes are **stereo**; mono sources are centered into that stereo bus. Native maximum is **24 physical channels**; an actual device exceeding 24 is **rejected**, not trimmed to 22.
- POC is limited to **2 active receivers** (bound separate from the future production default of 8).
- Receivers: iPhone Safari and Android Chrome, **wired outputs only**, page **visible and screen on**.
- Host network is **Ethernet or Wi-Fi** on the operator-selected interface; **VPN/public interfaces excluded by default**.
- Gains are **−60..0 dB in 1 dB steps, no positive boost**; mute is a **boolean logical −∞, never numeric JSON infinity**; malformed/non-finite values are **rejected** (never clamped to success), only valid finite out-of-range values are clamped to bounds and canonicalized.
- Unmute of a channel or master is **forbidden while unarmed**; mute is always allowed.
- The full **≤10 ms electrical target is not proved by any software metric**; only a physical input-reference vs headphone-output measurement can approach it. End-to-end display stays **"Not measured"**.
- UI diagnostics refresh **once per second**, independent of **10 Hz** meters and of the 500 ms progress watchdog / 1 s heartbeat with 3-miss lease.
- `frameCount` appears on the wire as a **decimal string constrained to the u32 range** (`FrameCountWire`); `hostEpoch`/`audioEpoch`/`sessionEpoch`/`armNonce`/`sourceId` are **opaque UUID strings** (non-decimal); `catalogRevision`/`mixRevision`/`safetyGeneration`/`startSample`/`channelMapRevision` are **decimal-only u64 strings** on the wire with native u64 internally. Guard JS parsing with `BigInt`.
- Realtime identities passed through frames are **`Copy` newtypes over `Uuid`/`u64`**, not heap strings; wire form is strings; formatting happens **off the audio thread**.
- Node is **build tooling only**; no runtime Node audio server. **Node >= 24.15** is the supported floor; the inspected machine (Node v24.20.0, npm 11.19.0) satisfies it and jsdom 30.1.2 is compatible with it. Do not require an exact macOS/npm patch version beyond the inspected machine.
- **Lockfiles and generated artifacts are normally repository artifacts.** They are generated during execution and **not committed/pushed without an explicit user request** (this session has none).
- **This repository currently has no `HEAD`**; do not create an initial commit, worktree, or git-config change to enable isolation. Disjoint file ownership across lanes is the isolation mechanism; no forced commits.
- **No automatic tool, certificate, or trust installs.** Installations and TLS root trust are explicit operator actions (Tasks 0 and 10).
- **All version numbers above are a source-supported candidate baseline frozen during execution, not a compatibility or build claim.** API integration dependencies are source-verified, not build-validated.
- **No per-lane commits or pushes.** The user has requested publication after implementation and verification: the integration owner reviews status/diffs/history, stages only intended non-secret files, commits, and pushes `main` to `https://github.com/daryllmagsombol/iem-cast.git`. Never force-push. This authorization does not bypass plan approval, verification, or the separate certificate-trust gate.

## Parallel Workgraph (frozen ownership)

The user selected **parallel agents**. No lane starts until the plan is reviewed. Task 0 and Task 1 are serial prerequisites. Tasks 2–7 run in parallel **only after** Task 1 freezes the shared contracts; each of Tasks 2–7 consumes **frozen contracts and test doubles/mocks**, never another lane's unimplemented private code.

```
0 -> 1 -> { A: 2 -> 3 , B: 4 , C: 5 -> 6 , D: 7 }
{ A,B,C,D } -> 8   (desktop composition + early mDNS interop probe)
8 -> 9             (automated integration/browser gate)
9 -> 10            (manual real-hardware qualification; may be blocked, never faked)
```

**Shared files — edited ONLY by the Integration (INT) owner and pre-declared in Task 1, NEVER edited by other lanes during parallel execution:** root `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, root `package.json`, `package-lock.json`, every crate/package `Cargo.toml`/`package.json`/`tsconfig.json`, all `vite.*.config.ts`/`vitest.config.ts` (and later `playwright.config.ts`), `tauri.conf.json`, `permissions/*`, `capabilities/*`, HTML `index.html` entrypoints, `apps/web/src/admin/main.tsx`, `apps/web/src/musician/main.tsx`, `apps/web/src/styles/app.css`, `apps/web/src/desktop-bridge/contracts.ts`, `apps/web/src/protocol/index.ts`, `crates/host-core/src/lib.rs`, `crates/host-core/src/contract.rs`, `crates/host-core/src/ids.rs`, and `apps/desktop/src-tauri/src/main.rs`/`lib.rs`. **Task 1 declares all `pub mod`s in `lib.rs` and creates a stub `mod.rs` in every future lane folder**, so lanes never edit `lib.rs` mid-parallel; each lane's private `mod.rs` stub is created by INT in Task 1 and **transferred to that lane** once Task 1 completes. Root `lib.rs` stays INT-frozen for the whole parallel phase.

| Lane | Owner type | Owned private paths (exclusive after Task 1) | Tasks |
| --- | --- | --- | --- |
| INT | integration/tooling/contract | all shared files above; `docs/qualification/macos-poc.md` | 0, 1, 8, 9, 10 |
| A | fixer | `crates/host-core/src/capture/**`, `src/audio/**`, `src/encoder/**` + their tests | 2, 3 |
| B | fixer | `crates/host-core/src/server/**`, `src/control/**`, `src/config/**` + their tests | 4 |
| C | fixer | `crates/host-core/src/transport/**`; `apps/web/src/transport/**` + their tests | 5, 6 |
| D | designer | `apps/web/src/ui/**`, `apps/web/src/styles/tokens.css`, `apps/web/src/admin/AdminRoot.tsx` + `apps/web/src/admin/*View.tsx`, `apps/web/src/musician/MusicianRoot.tsx` + `apps/web/src/musician/*View.tsx` + view tests | 7 |

**Ownership transfer:** Task 1 creates minimal `AdminRoot.tsx`/`MusicianRoot.tsx` stubs (and an empty `tokens.css` if `app.css` imports it) so the Task 1 build compiles. From Task 7 onward Lane D owns `AdminRoot.tsx`/`MusicianRoot.tsx`, `tokens.css`, and the view files, replacing the stubs. Task 1 also creates each lane's `mod.rs` **stub** and declares it in `lib.rs`; once Task 1 completes, the private `mod.rs` for each lane is **transferred to that lane's owner** (it is no longer INT-owned), while root `lib.rs` stays INT-frozen for the entire parallel phase. `main.tsx` entrypoints stay INT-owned and import `AdminRoot`/`MusicianRoot`. No two lanes edit the same file at the same time; no overlapping test filenames. Task 7 tests use **mock controllers only**.

## Review Focus

The five input classes most likely to bite a user, each pinned to owning-task tests with exact names:

1. **Stop sent before the arm reply** — a person expects immediate personal silence and no later re-arm from a stale reply. Pinned in Task 4 (`stale_generation_disarm_for_authenticated_current_session_is_applied`, `stale_worker_cannot_arm_after_disarm`) and Task 6 (`stop_is_immediate_without_ack_and_invalidates_nonce`, `unmute_false_requires_armed_current_readiness_and_gesture`).
2. **Capture device gone, block dropped, or timeline gap** — a person expects no backlog replay and an explicit re-arm. Pinned in Task 2 (`pool_conservation_counts_in_flight_slots`, `drop_advances_timeline_and_counts_dropped_frames`, `device_loss_via_fake_backend_emits_fault_and_new_audio_epoch`).
3. **mDNS candidate failure or a mono fmtp answer** — a person expects a visible refusal, not a silent downmix or an insecure workaround. Pinned in Task 5 (`mdn_resolution_failure_is_visible_and_escalates`, `mono_fmtp_is_explicitly_refused`, `valid_selected_lan_ip_candidate_is_accepted_without_resolver_call`).
4. **Cross-musician access and shared-source mixing** — a person expects two musicians can use one shared USB source with independent settings, and cannot address another session or an unavailable source. Pinned in Task 4 (`two_musicians_can_listen_to_the_same_source_with_independent_settings`, `unavailable_source_is_forbidden_within_logical_permissions`, `cross_mix_session_id_is_rejected_at_the_server_boundary`).
5. **Missing/reset/cached stats, fake zero, and narrow-screen usability** — a person expects "Unavailable"/"Waiting for sample" rather than fabricated zero, and a usable page. Pinned in Task 6 (`diagnostics_show_unavailable_never_zero`, `buffer_delay_first_sample_shows_waiting_for_sample`, `buffer_delay_zero_emitted_delta_is_unavailable`) and Task 9 (`mobile_layout_has_no_horizontal_overflow`, `keyboard_focus_is_visible`).

**Test-helper convention (no unspecified dead ends):** every test-private helper referenced in a task (`capture_req_48k`, `start_with_backend`, `two_channel_map`, `capture_block_constant`, `ctx_a`, `snapshot_gain`, `listen_arm_with_gen`, `listen_arm_with_nonce`, `sess`, `shared_source`, `unavailable_source`, `control_actor`, `PairStore`, `test_entropy`, `test_clock`, `test_log_sink`, `issue_cookie`, `origin_gate`, `test_server_with_two_pairings`, `media_session`, `offer_with_pt`, `recvonly_audio_offer`, `offer_with_video`, `offer_mono_fmtp`, `MdnsAdapter`, `render_one_frame`, `render_unity_correlated`, `test_stereo_frame_120`, `composed_host_with_fake_capture`, `buildDiagnostics`, and the TS `fakePorts`/`fakeBridge`/`mockStrip`/`diagSnapshot`/`flushEvents`/`startFakeHost`) is defined **in that task's own private test module** (`tests.rs`, `__tests__/helpers.ts`, or `e2e/fixtures/fakeHost.ts`) with a single fixture purpose, and is **not** a production API. Behaviors that need many inputs use the compact case tables in Tasks 3/6/9 rather than extra helpers. Task 3's geometry test uses real production calls (`AudioEngine::new`/`register_session`/`process_block`), not a helper.

---

### Task 0: Prerequisite and approval gate

**Files:**
- Create: `docs/qualification/macos-poc.md` (prerequisite section only)

**Interfaces:**
- Consumes: nothing.
- Produces: recorded tool versions and approval state; no code.

- [ ] **Step 1: Record detected environment**

Run: `node -v && npm -v && sw_vers -productVersion && uname -m && xcode-select -p`
Expected: prints Node 24.x, npm 11.x, macOS 27.x, arm64, and an Xcode CLT path. (Inspected: Node v24.20.0, npm 11.19.0, macOS 27.0, arm64.)

- [ ] **Step 2: Check required toolchain presence**

Run: `for t in rustc cargo cmake mkcert clang; do command -v $t || echo "$t MISSING"; done`
Expected: `clang` present; `rustc`, `cargo`, `cmake`, `mkcert` may print `MISSING`. **Absence on `PATH` is not proof of absence from the machine** — verify with the installer's own version command before concluding.

- [ ] **Step 3: Request the install/approval gate (explicit operator action)**

Record in `docs/qualification/macos-poc.md` and request **explicit user approval** before installing anything: Rust >= 1.90 (rustup), CMake and a Clang toolchain (for the bundled libopus static build), and mkcert (local TLS generation). State that **no install is automatic** and that the Rust minimum pin becomes `rust-toolchain.toml` in Task 1 only after approval.

- [ ] **Step 4: Request certificate trust as a separate, later action**

Record that **mkcert root-CA trust is a separate manual operator action for devices in Task 10** and is never installed at task start. No admin/root trust changes are automated.

- [ ] **Step 5: Confirm approval recorded**

Expected: an explicit approval note (and any declined items) in `docs/qualification/macos-poc.md`. If the toolchain is not approved/present, Tasks 1–9 cannot start; Task 10 additionally requires hardware.

### Task 1: Workspace, frozen contracts, fixtures, and shared seams

**Files:**
- Create (INT): `Cargo.toml`, `rust-toolchain.toml`, `package.json`, `apps/web/package.json`, `apps/desktop/package.json`, `apps/web/tsconfig.json`, `apps/desktop/tsconfig.json`
- Create (INT): `apps/web/vite.admin.config.ts`, `apps/web/vite.musician.config.ts`, `apps/web/vitest.config.ts`, `apps/web/admin/index.html`, `apps/web/musician/index.html`, `apps/web/src/admin/main.tsx`, `apps/web/src/admin/AdminRoot.tsx` (stub), `apps/web/src/musician/main.tsx`, `apps/web/src/musician/MusicianRoot.tsx` (stub), `apps/web/src/styles/app.css`
- Create (INT): `crates/host-core/Cargo.toml`, `crates/host-core/src/lib.rs`, `src/ids.rs`, `src/contract.rs`, `src/fixtures.rs`, and `mod.rs` stubs at `src/capture/mod.rs`, `src/audio/mod.rs`, `src/encoder/mod.rs`, `src/server/mod.rs`, `src/control/mod.rs`, `src/transport/mod.rs`, `src/config/mod.rs`, `src/diagnostics/mod.rs`
- Create (INT): `crates/host-core/tests/contract_serde.rs`, `crates/host-core/fixtures/mix.patch.json`, `catalog.snapshot.json`, `mix.applied.json`, `arm.json`
- Create (INT): `apps/web/src/protocol/index.ts`, `apps/web/src/desktop-bridge/contracts.ts`, `apps/web/src/receiver-ports.ts`, `apps/web/src/protocol/__tests__/envelope.test.ts`, `apps/web/src/fixtures.ts`
- Create (INT): `apps/desktop/src-tauri/Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/bridge.rs` (HostBridge + `issue_pairing_credential` command stubs), `tauri.conf.json`, `permissions/operator.toml` (name list; values filled in Task 8)

**Interfaces:**
- Consumes: Task 0 approval.
- Produces (frozen; every other task uses these exact names):
  - Realtime identity newtypes (Copy; wire strings; format off-thread): `HostEpoch(Uuid)`, `AudioEpoch(Uuid)`, `SessionEpoch(Uuid)`, `ArmNonce(Uuid)`, `SourceId(Uuid)`, each `Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug` and serde-as-UUID-string. `HostEpoch` is global control identity and is **not carried in audio frames**. Wire fixtures and examples use valid UUIDs, e.g. `11111111-1111-4111-8111-111111111111`; do **not** use `host-epoch-example` if the serializer parses UUIDs (it would fail).
  - Counter newtypes (Copy, u64, serde decimal string): `CatalogRevision`, `MixRevision`, `SafetyGeneration`, `ChannelMapRevision`. On the wire these stay **decimal strings**; the TS side may define a branded `CounterString = string` and convert to `bigint` only for validation/comparison. **Never return a `bigint` from `parseEnvelope`** — the parsed object must stay JSON-serializable and preserve `"120"` verbatim.
  - `FrameCountWire(pub u32)` with serde as a decimal string and **rejection above `u32::MAX`**; `RequestId(pub String)` (opaque; not a realtime hot path).
  - Native constructors used by tests are declared here so no task invents them: `SourceId::from_bytes([u8; 16]) -> SourceId`, `AudioEpoch::from_bytes([u8; 16]) -> AudioEpoch`, `StereoFrame::zeroed(context: SessionContext) -> StereoFrame`, `EncodedFrame::zeroed() -> EncodedFrame`, `SourceGainMatrix::from_slice(&[SourceGain]) -> SourceGainMatrix`.
  - Wire DTOs use **camelCase** on the wire (`#[serde(rename_all = "camelCase")]`) matching the ARCHITECTURE `mix.patch` example; native structs keep `snake_case` internally and map explicitly. `MixAck` wire fields are `acceptedRevision`, `catalogRevision`, `canonicalSettings` (from native `canonical_snapshot`). `MixApplied` wire fields are `appliedRevision`, `audioEpoch`, `startSample`, `settings` (from native `context`/`snapshot` via an explicit adapter, not a nested `context`). `ListenArm` wire fields are `audioEpoch`, `safetyGeneration`, `appliedRevision`, `armNonce`; the native `SessionContext` is internal only.
  - `SessionContext { session_epoch: SessionEpoch, audio_epoch: AudioEpoch, safety_generation: SafetyGeneration }` (`Copy`), carried by `MixSnapshot`, `StereoFrame`, `EncodedFrame`, and the `AudioEvent` variants so one `ControlActor` can route two sessions.
  - Fixed storage: `AudioSlot = Box<[f32; 256 * 24]>` (moved through SPSC, never reallocated); `StereoPcm = [f32; 480]` (max 240 stereo frames interleaved, enough for the later 5 ms/240 probe; initial is 120); `BoundedPacket { len: u16, bytes: [u8; 1275] }`.
  - `SampleFormat { F32, I16, U16 }` (own small wire enum summarizing CPAL) and `EntropyError { Unavailable }`; `DeviceInfo { device_id: String, name: String, is_default: bool, input_channels: u16, sample_formats: Vec<SampleFormat>, sample_rate_hz: Option<u32>, buffer_min_frames: Option<u32>, buffer_max_frames: Option<u32> }`. `sample_rate_hz` is `Some(48000)` only when the actual selected device's supported configs include 48 kHz; it is never a fabricated `[48000]` whitelist array, and an unknown/empty set is `None`.
  - `CaptureRequest { device_id: String, sample_rate_hz: u32, buffer_frames: u32 }`; `CaptureBlock { audio_epoch: AudioEpoch, start_sample: u64, frame_count: u32, sample_rate_hz: u32, channel_count: u16, channel_map_revision: ChannelMapRevision, samples: AudioSlot }`.
  - `ChannelMapEntry { physical_index: u16, source_id: SourceId, stereo_pair: Option<SourceId>, role: SourceRole }`; `SourceRole { InputChannel, MasterLr }`; `Config { version: u32, device_id: Option<String>, selected_interface: Option<String>, channel_map: Vec<ChannelMapEntry>, labels: Vec<(SourceId, String)> }`.
  - `SourceInfo { source_id: SourceId, physical_index: u16, label: String, role: SourceRole, authorized: bool, available: bool, stereo_pair: Option<SourceId> }`; `SourceGain { source_id: SourceId, gain_db: f32, muted: bool }` (`Copy`, no `String`); `SourceGainMatrix { entries: [SourceGain; 24], len: u16 }` (native fixed matrix, serde as sequence).
  - `MixSnapshot { context: SessionContext, catalog_revision: CatalogRevision, mix_revision: MixRevision, sources: SourceGainMatrix, master_db: f32, master_muted: bool }`.
  - `StereoFrame { context: SessionContext, start_sample: u64, frame_count: u32, pcm: StereoPcm }`; `EncodedFrame { context: SessionContext, start_sample: u64, frame_count: u32, enqueue_instant: Instant, packet: BoundedPacket }`.
  - `AudioEvent { MixApplied { applied_revision: MixRevision, context: SessionContext, start_sample: u64, snapshot: MixSnapshot }, ArmApplied { context: SessionContext, arm_nonce: ArmNonce }, Interrupted { session_epoch: SessionEpoch, reason: InterruptReason }, Fault { code: FaultCode } }`.
  - `MediaCommand { Offer { session_epoch: SessionEpoch, sdp: String }, Candidate { session_epoch: SessionEpoch, candidate: Option<String> }, Attach { session_epoch: SessionEpoch }, Detach { session_epoch: SessionEpoch } }` (`candidate: None` = end-of-candidates).
  - `MediaEvent { Answer { session_epoch: SessionEpoch, sdp: String }, Readiness { session_epoch: SessionEpoch, ready: bool }, Failure { session_epoch: SessionEpoch, code: MediaFailureCode } }`.
  - Concrete enums: `CaptureFault { DeviceUnavailable, UnsupportedRate, UnsupportedFormat, ChannelCountTooLarge, StreamError, Disconnected }`; `AudioFault { Internal, InvalidSample, RevisionStale, UnmappedSource }`; `EncodeFault { EncoderInit, EncodeFailed }`; `ControlError { RevisionConflict, SourceForbidden, StaleEpoch, Unauthorized, UnarmedUnmuteForbidden, RateLimited, TlsIdentityInvalid, Internal }`; `InterruptReason { QueueResidenceExceeded, DeviceLost, NetworkDisconnected, OutputRouteChange, UserDisarm }`; `FaultCode { Capture(CaptureFault), Audio(AudioFault) }`; `MediaFailureCode { NegotiationFailed, IceFailed, MdnUnresolved, NoAudioSection, ExtraMediaRejected, PeerDisconnected }`. The five structured error strings (`REVISION_CONFLICT`, `SOURCE_FORBIDDEN`, `STALE_EPOCH`, `CAPTURE_FAULT`, `TLS_IDENTITY_INVALID`) map from these variants; RTP jitter/concealment/loss are statistics, not variants.
  - Wire DTOs: `EnvelopeV1<T> { v: u8, #[serde(rename = "type")] kind: String, request_id: RequestId, host_epoch: HostEpoch, session_epoch: Option<SessionEpoch>, payload: T }`; `MixPatch { base_revision: MixRevision, catalog_revision: CatalogRevision, sources: SourceGainMatrix, master_db: f32, master_muted: bool }`; `MixAck { accepted_revision: MixRevision, catalog_revision: CatalogRevision, canonical_snapshot: MixSnapshot }`; `MixApplied { applied_revision: MixRevision, context: SessionContext, start_sample: u64, snapshot: MixSnapshot }`; `ListenArm { context: SessionContext, applied_revision: MixRevision, arm_nonce: ArmNonce }`; `ListenArmed { session_epoch: SessionEpoch, safety_generation: SafetyGeneration, audio_epoch: AudioEpoch, arm_nonce: ArmNonce }`; `ListenDisarm { session_epoch: SessionEpoch, safety_generation: SafetyGeneration }`; `SessionSnapshot { session_epoch: SessionEpoch, context: SessionContext, phase: ListenerPhase, catalog_revision: CatalogRevision, requested_mix: Option<MixSnapshot>, accepted_mix: Option<MixSnapshot>, applied_mix: Option<MixSnapshot> }`; `CatalogSnapshot { catalog_revision: CatalogRevision, sources: Vec<SourceInfo> }`; `ListenerState { session_epoch: SessionEpoch, phase: ListenerPhase, armed: bool }`; `ListenerPhase { Unpaired, Paired, Negotiating, ReadyMuted, Armed, Interrupted, Revoked }`.
  - `InterfaceInfo { name: String, ip_address: IpAddr, prefix: u8 }`; `StartHostRequest { capture: CaptureRequest, interface: InterfaceInfo, certificate_path: String, key_path: String }`; `StartHostResult { host_epoch: HostEpoch, audio_epoch: AudioEpoch, join_url: String, catalog: CatalogSnapshot }`.
  - Injection traits (needed by Task 4 in parallel), all `Send + Sync`: `trait Clock { fn now(&self) -> Instant; }`, `trait Entropy { fn fill(&self, buf: &mut [u8]) -> Result<(), EntropyError>; }` (randomness failure is explicit, never silently falls back), `trait AssetProvider { fn get(&self, path: &str) -> Option<&'static [u8]>; }`.
  - Control→DSP and DSP→control are **separate** (the current plan had one `DspPublisher` pointing the wrong way): `trait DspControl { fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError>; fn request_arm(&self, arm: ListenArm) -> Result<(), ControlError>; fn disarm(&self, session: SessionEpoch, new_generation: SafetyGeneration); }` (priority: `disarm` closes the shared safety gate immediately). `trait AudioEventSink { fn try_publish(&self, ev: AudioEvent) -> bool; }` (bounded, non-blocking; the audio worker calls it **only after applying**). There is no `AudioEvent::MixApplied` constructor ambiguity: the DSP worker builds the enum variant directly from applied state.
  - `ControlActor::new(clock: Arc<dyn Clock>, dsp: Arc<dyn DspControl>, catalog: CatalogSnapshot, host_epoch: HostEpoch, audio_epoch: AudioEpoch) -> ControlActor`, with `pub fn current_generation(&self, session: SessionEpoch) -> SafetyGeneration;` (per-session getter). The actor assigns generations; the per-listener `SafetyWorker` **consumes the provided generation and never assigns its own counter**. Include `TlsIdentityInvalid` in the Task 1 `ControlError` enum, as required by Task 4.
  - `HostBridge` (Tauri **operator** IPC only, TS mirror): `listDevices(): Promise<DeviceInfo[]>`, `listInterfaces(): Promise<InterfaceInfo[]>`, `startHost(req: StartHostRequest): Promise<StartHostResult>`, `stopHost(): Promise<void>`, `sourceCatalog(): Promise<SourceInfo[]>`, `setAvailableSources(ids: string[]): Promise<CatalogSnapshot>`, `setSourceLabel(id: string, label: string): Promise<CatalogSnapshot>`, `issuePairingCredential(): Promise<PairingCredential>`. `setAvailableSources` lets the operator **choose** the catalog; `issuePairingCredential` is the approved QR-join operator path (a fresh single-use credential per phone; the first two phones cannot share one one-use QR). `PairingCredential { joinUrl: string, expiresInSeconds: number }` (the one-use fragment token is embedded in `joinUrl`, generated by the host `PairStore`, never logged).
  - `ReceiverController` is created via **ports** so the LAN musician bundle never imports `@tauri-apps/api`: `createReceiverController(ports: ReceiverPorts): ReceiverController;` where `ReceiverPorts { signaling: MusicianSignaling; media: AudioMediaPort; activation: ActivationPort; clock: () => number; stats: StatsPort; }`. Minimal port methods: `MusicianSignaling.connect(): Promise<void>; requestMix(patch: MixPatch): Promise<MixAck>; arm(arm: ListenArm): Promise<void>; disarm(): Promise<void>; onEvent(h: (ev: ServerEvent) => void): () => void;` (authenticated musician WSS). `AudioMediaPort.createRecvOnlyAudio(): Promise<void>; setJitterBufferTargetMs(ms: number): Promise<boolean>; play(): Promise<void>; stop(): void; mute(muted: boolean): void;` (browser WebRTC + media element). `ActivationPort.onUserGesture(h: () => void): () => void;`. `StatsPort.selectedCandidateRtt(): { currentRoundTripTime: number } | undefined; inbound(): InboundStats | undefined;`. DI mocks for tests live in the test helper files; **no `@tauri-apps/api` import enters the LAN bundle**.
  - `ReceiverController` interface (implementation Task 6, declared in `apps/web/src/protocol/index.ts`): `subscribe(h: (s: ReceiverSnapshot) => void): () => void`, `getSnapshot(): ReceiverSnapshot`, `connect(): Promise<void>`, `requestMix(patch: MixPatch): Promise<MixAck>` (**resolves on accepted, never on DSP-applied**), `arm(): Promise<void>` (**invokes `media.play()` from the user gesture BEFORE awaiting host confirmation**), `personalMasterMute(muted: boolean): void` (see §Task 6 for exact semantics), `stop(): void`, `disconnect(): void`. `ReceiverSnapshot { phase: ListenerPhase, catalog: CatalogSnapshot, requested_mix: MixSnapshot | null, accepted_mix: MixSnapshot | null, applied_mix: MixSnapshot | null, master_local_muted: boolean, diagnostics: DiagnosticsSnapshot, error: string | null }`; `DiagnosticsSnapshot { network_rtt_ms: number | 'Unavailable', buffer_delay_ms: number | 'Unavailable' | 'Waiting for sample', end_to_end: 'Not measured', last_updated: number | null }`. Native mix structs stay `snake_case`; the DTO wire uses camelCase. TS phone views must **not** implement the safety gate independently.
  - Complete the ports' named types in Task 1: `ServerEvent` is a discriminated union of v1 envelopes for the server message types in ARCHITECTURE §12; their payloads use the wire DTOs above. Define remaining payloads as `RtcAnswer { sdp: string }`, `RtcCandidate { candidate: string | null; sdpMid?: string; sdpMLineIndex?: number }`, `ProtocolError { code: string; message: string; retryable: boolean }`, and `Telemetry { audioEpoch: string; startSample: CounterString; sourcePeaksDbfs: Record<string, number | null>; outputPeaksDbfs: [number | null, number | null]; limiterReductionDb: number | null }`. Server events are complete envelopes, not unvalidated partial payloads.
  - `InboundStats { sessionEpoch: string; statsId: string; stream: string; epoch: string; emitted: number; delay: number; sampledAtMs: number }` is the normalized browser sample, with `delay` in seconds. `computeBufferDelayMs(previous: InboundStats | null, current: InboundStats | undefined)` returns a finite number or the documented unavailable/waiting label. Add `ActivationPort.isActive(): boolean` so `arm()` can check the current trusted gesture. Serialize `ListenerPhase` using kebab-case (`ready-muted`, not `ReadyMuted`).
  - Add `StatsPort.refresh(): Promise<void>` to collect `getStats()` asynchronously before reading its cached normalized samples; do not overlap refreshes. Test-only `emitServerEvent` helpers construct complete current-session envelopes from fixtures before applying payload overrides. Stale-nonce tests use a valid UUID belonging to a cancelled attempt, with the other identity fields current, rather than relying on malformed-input rejection.
  - npm scripts (defined here; every later `Run:` uses only these). Root `package.json`: `"typecheck": "npm run typecheck --workspace apps/web"`, `"test": "npm run test --workspace apps/web"`, `"build:web": "npm run build --workspace apps/web"`, `"build:desktop": "npm run build:web && npm run tauri --workspace apps/desktop -- build --no-bundle"`, `"dev:admin": "npm run dev:admin --workspace apps/web"`, `"dev:desktop": "npm run build:web && npm run tauri --workspace apps/desktop -- dev"`, `"test:e2e": "playwright test"`. `apps/web/package.json`: `"test": "vitest run"`, `"typecheck": "tsc --noEmit"`, `"build": "npm run typecheck && npm run build:admin && npm run build:musician"`, `"build:admin": "vite build --config vite.admin.config.ts"`, `"build:musician": "vite build --config vite.musician.config.ts"`, `"dev:admin": "vite --config vite.admin.config.ts"`. `apps/desktop/package.json`: `"tauri": "tauri"`.
  - Root `Cargo.toml` sets `default-members = ["crates/host-core"]` and includes `apps/desktop/src-tauri` as a workspace member, so bare `cargo test`/`cargo clippy` never build the Tauri crate before web assets exist.
  - Shared TS contract in `apps/web/src/desktop-bridge/contracts.ts` and `apps/web/src/protocol/index.ts`. React UI test helpers are **declared per-task in private helper files**, not invented ad hoc: this plan uses compact **case tables** (action → exact output → key assertion) rather than many toy helper functions, so every helper referenced is either a Task 1 static method (above), a per-task private helper named in that task, or a literal fixture defined once in `crates/host-core/fixtures/`/`apps/web/src/fixtures.ts`.

- [ ] **Step 1: Write the failing Rust serde test**

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p host-core --test contract_serde`
Expected: FAIL to compile with "cannot find type `EnvelopeV1`" (the contract is not yet implemented). A compile-time failure is acceptable at this stage; once the harness compiles, the assertions above must hold.

- [ ] **Step 3: Implement the contract, identity newtypes, fixtures, and lane stubs**

Implement `ids.rs` (UUID/counter newtypes with the serde behavior above) and `contract.rs` (all types, camelCase wire mapping and explicit native↔wire adapters). Declare all `pub mod` in `src/lib.rs` and create each lane-folder `mod.rs` stub. Add `crates/host-core/fixtures/mix.patch.json`, `catalog.snapshot.json`, `mix.applied.json`, `arm.json` with the wire shapes from ARCHITECTURE §12 (values include `baseRevision "120"`, `catalogRevision "34"`, `gainDb -12`, `masterDb -6`, and valid UUID strings). Add `src/fixtures.rs` with the shared fixture loader used by both Rust and TS tests.

- [ ] **Step 4: Run the Rust test to verify it passes**

Run: `cargo test -p host-core --test contract_serde`
Expected: PASS.

- [ ] **Step 5: Write the failing TS envelope test**

```ts
// apps/web/src/protocol/__tests__/envelope.test.ts
import { parseEnvelope, parseFrameCount } from '../index';
import mixPatchFixture from '../../../../../crates/host-core/fixtures/mix.patch.json';

test('parseEnvelope preserves wire type and decimal revision strings and stays serializable', () => {
  const parsed = parseEnvelope(mixPatchFixture);
  expect(parsed.type).toBe('mix.patch');
  expect(parsed.payload.baseRevision).toBe('120');           // wire string preserved
  expect(BigInt(parsed.payload.baseRevision)).toBe(120n);    // BigInt only for comparison
  expect(parsed.payload.sources[0].sourceId).toBe('33333333-3333-4333-8333-333333333333');
  expect(() => JSON.stringify(parsed)).not.toThrow();
  expect(JSON.stringify(parsed)).toContain('"120"');
});

test('parseFrameCount parses a u32 number and rejects larger values', () => {
  expect(parseFrameCount('240')).toBe(240);
  expect(() => parseFrameCount('4294967296')).toThrow(/u32/);
});
```

- [ ] **Step 6: Run the TS test to verify it fails**

Run: `npm run test --workspace apps/web`
Expected: FAIL with "Cannot find module '../index'" (implementation not written yet).

- [ ] **Step 7: Implement `parseEnvelope` and `parseFrameCount`**

`parseEnvelope` keeps the wire key `type`, preserves decimal revision strings verbatim (branded `CounterString`), uses `BigInt` only to validate/comparison, and never emits a `bigint` (so `JSON.stringify` works). `parseFrameCount` parses with `BigInt`, rejects non-decimal and values `> 4294967295`, and returns a `number`.

- [ ] **Step 8: Run the TS test to verify it passes**

Run: `npm run test --workspace apps/web`
Expected: PASS.

- [ ] **Step 9: Run the Task 1 host-core and frontend gates**

Run: `cargo clippy -p host-core --all-targets -- -D warnings`
Expected: PASS, no warnings.
Run: `cargo test -p host-core`
Expected: PASS (all host-core tests, including `contract_serde`).
Run: `npm run typecheck --workspace apps/web`
Expected: PASS.
Run: `npm run test --workspace apps/web`
Expected: PASS.

### Task 2: CoreAudio capture pool and source timeline

**Files:**
- Create: `crates/host-core/src/capture/device.rs`, `capture/pool.rs`, `capture/timeline.rs`, `capture/service.rs`
- Modify (Lane A only): `crates/host-core/src/capture/mod.rs` (replace stub with real `pub use`)
- Test: `crates/host-core/src/capture/tests.rs`

**Interfaces:**
- Consumes: `DeviceInfo`, `SampleFormat`, `CaptureRequest`, `CaptureBlock`, `AudioSlot`, `AudioEpoch`, `ChannelMapRevision`, `CaptureFault` (Task 1); `rtrb` 0.4.0.
- Produces:
  - `pub const CAPTURE_SLOTS: usize = 8; pub const MAX_BLOCK_FRAMES: usize = 256;`
  - `pub fn enumerate_input_devices() -> Vec<DeviceInfo>`
  - `pub struct CaptureTelemetry { pub latest_position: AtomicU64, pub dropped_frames: AtomicU64, pub dropped_blocks: AtomicU64, pub fault: AtomicU8 }`
  - `pub struct CaptureHandle { pub ready_rx: rtrb::Consumer<CaptureBlock>, pub free_tx: rtrb::Producer<AudioSlot>, pub telemetry: Arc<CaptureTelemetry>, pub configured_channels: u16, pub sample_rate_hz: u32, pub stop: StopHandle }` — the **DSP side** consumes `ready_rx` and produces into `free_tx`.
  - `pub struct StopHandle; pub struct CaptureService;` (opaque lifecycle owners)
  - `pub fn start(req: CaptureRequest, audio_epoch: AudioEpoch, channel_map_revision: ChannelMapRevision) -> Result<CaptureHandle, CaptureFault>`
  - The **callback owns `free_rx` and `ready_tx` privately on the capture thread**: it is the free-slot consumer and ready-slot producer. A producer never removes ready entries from the consumer-owned side.

- [ ] **Step 1: Write the failing pool/timeline tests**

```rust
#[test]
fn pool_conservation_counts_in_flight_slots() {
    let mut pool = CapturePool::new();
    let a = pool.acquire_free().expect("free slot");
    let _ = pool.publish(a, 128usize);
    let _c = pool.callback_hold();
    let _d = pool.dsp_hold();
    assert_eq!(pool.free_count() + pool.ready_count() + pool.callback_held() + pool.dsp_held(), 8);
}

#[test]
fn drop_advances_timeline_and_counts_dropped_frames() {
    let mut tl = Timeline::new();
    let before = tl.latest_position();
    tl.on_drop(128);
    assert_eq!(tl.latest_position(), before + 128);
    assert_eq!(tl.dropped_frames(), 128);
}

#[test]
fn block_ending_over_4ms_behind_latest_is_discarded() {
    assert!(should_discard(/* block_end */ 100, /* latest */ 100 + 4 * 48 + 1));
    assert!(!should_discard(100 + 4 * 48, 100 + 4 * 48));
}

#[test]
fn captured_chunk_is_the_real_callback_width_not_forced_256() {
    let mut pool = CapturePool::new();
    let s = pool.acquire_free().unwrap();
    let fc = pool.publish(s, 128usize);
    assert_eq!(fc, 128); // 64/128 chunk preserved; internal counters are native usize, not FrameCountWire
}

#[test]
fn device_loss_via_fake_backend_emits_fault_and_new_audio_epoch() {
    let backend = FakeCaptureBackend::new();
    let h = start_with_backend(&backend, capture_req_48k(), AudioEpoch::from_bytes([7; 16]), ChannelMapRevision(1)).unwrap();
    backend.inject_disconnect();
    assert_eq!(h.telemetry.fault(), FaultCode::Capture(CaptureFault::Disconnected));
    assert_ne!(next_audio_epoch(AudioEpoch::from_bytes([7; 16])), AudioEpoch::from_bytes([7; 16]));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p host-core capture::tests`
Expected: FAIL to compile with "cannot find function `enumerate_input_devices`" or "cannot find type `CapturePool`".

- [ ] **Step 3: Implement capture against cpal (source-supported calls)**

Use only these confirmed calls: `device.id()`, `device.description()` (`DeviceDescription`), `device.supported_input_configs()` (channel count, `SampleFormat`, `SupportedBufferSize`), `device.default_input_config()`, `config.sample_rate()` (a `u32` alias), and `supported.try_with_sample_rate(48_000)` — **no `SampleRate(48000)` constructor**. Build with `device.build_input_stream::<f32, _, _>(config-by-value, data_callback, error_callback, timeout)`. Map the **actual** supported layout; request the full supported input layout and verify mapping physically. A device reporting **more than 24 input channels is rejected** (`ChannelCountTooLarge`), not trimmed. Prefer `buffer_frames` 64 or 128 only when actually supported; split callbacks to at most 256 frames. `CaptureRequest.sample_rate_hz` must be a supported value (POC whitelist `[48000]`); otherwise `UnsupportedRate`. The callback performs only bounded copying, atomic counter updates, and non-blocking `rtrb` SPSC operations — no allocation, logging, locks, disk, or network. `CaptureBlock.frame_count` is the actual callback chunk (64/128/etc.), not forced to full 256 slots. On device error, set the telemetry fault and stop publishing; on restart/mapping/rate change, mint a new opaque `AudioEpoch` and reset partial assembly with no backlog. **Unit tests must use a `FakeCaptureBackend` fixture and never open real hardware**; only the ignored test in Step 5 touches a device.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p host-core capture::tests`
Expected: PASS.

- [ ] **Step 5: Add the hardware-gated device test (skipped by default)**

```rust
#[test]
#[ignore = "requires a connected Soundcraft device; run manually only in Task 10"]
fn coreaudio_lists_the_selected_device_and_accepts_48000() {
    let id = std::env::var("IEM_TEST_DEVICE_ID").expect("set IEM_TEST_DEVICE_ID to the Soundcraft device id");
    let d = enumerate_input_devices().into_iter().find(|d| d.device_id == id).expect("selected device");
    assert_eq!(d.sample_rate_hz, Some(48000)); // passes only for the operator-selected real device
}
```
Run: `IEM_TEST_DEVICE_ID=<id> cargo test -p host-core capture::tests -- --ignored` **only** in Task 10 with hardware attached.
Expected: PASS for the selected Soundcraft device; otherwise the test is not claimed.

- [ ] **Step 6: Run the capture suite and clippy**

Run: `cargo test -p host-core capture::tests`
Expected: PASS.
Run: `cargo clippy -p host-core --all-targets -- -D warnings`
Expected: PASS.

### Task 3: DSP, personal mixes, Opus, and safety-worker applied events

**Files:**
- Create: `crates/host-core/src/audio/gain.rs`, `audio/limiter.rs`, `audio/engine.rs`, `audio/safety.rs`
- Create: `crates/host-core/src/encoder/opus_worker.rs`, `encoder/rtp.rs`
- Modify (Lane A only): `crates/host-core/src/audio/mod.rs`, `crates/host-core/src/encoder/mod.rs` (replace stubs)
- Test: `crates/host-core/src/audio/tests.rs`, `crates/host-core/src/encoder/tests.rs`

**Interfaces:**
- Consumes: `CaptureBlock`, `MixSnapshot`, `SourceGainMatrix`, `SessionContext`, `AudioFault`, `StereoFrame`, `StereoPcm`, `EncodedFrame`, `BoundedPacket`, `EncodeFault`, `SafetyGeneration`, `ArmNonce`, `ListenArm`, `MixRevision`, `AudioEvent`, `ChannelMapEntry` (Task 1); `CaptureHandle` (Task 2).
- Produces:
  - `pub const BUS_HEADROOM_DB: f32 = -27.0;` (renamed from the earlier `WARNING_RANGES`; probe-only, clearly low volume, requires validation).
  - `pub const CODEC_FRAME_FRAMES: u32 = 120;` (initial 2.5 ms); `pub const MAX_OUT_FRAMES_PER_BLOCK: usize = 3;`.
  - `pub fn gain_linear(db: f32) -> f32` (`10f32.powf(db / 20.0)`).
  - `pub fn validate_gain(db: f32) -> Result<f32, AudioFault>` (reject non-finite; clamp only finite out-of-range to `[-60, 0]`; canonicalize).
  - `pub fn centered_coefficients() -> [f32; 2]` (returns `[1.0, 1.0]`).
  - `pub enum RtpFrameMode { Ms2p5, Ms5 }`; `pub fn rtp_step_for(mode: RtpFrameMode) -> u32` (120 / 240).
  - `pub struct AudioEngine;` with `pub fn new(sample_rate_hz: u32, channel_map: &[ChannelMapEntry]) -> Self;`, `pub fn register_session(&mut self, slot: usize, ctx: SessionContext, mix: MixSnapshot);`, and — because input blocks are up to 256 frames while a codec frame is 120 — `pub fn process_block(&mut self, slot: usize, input: &CaptureBlock, snapshot: &MixSnapshot, out: &mut [StereoFrame; MAX_OUT_FRAMES_PER_BLOCK]) -> Result<usize, AudioFault>;` (returns the number of 120-frame stereo frames produced this call; a partial frame is carried across calls; **per-session partial state is reset on `audioEpoch`/`safetyGeneration` change**). This replaces the invalid one-input→one-output `mix_block(frame_count == input.frame_count)`.
  - `pub struct SafetyWorker;` (one per listener, **consumes** the generation the actor provides) with `pub fn new(session: SessionEpoch) -> Self;`, `pub fn apply_arm(&self, arm: &ListenArm, generation: SafetyGeneration) -> Result<AudioEvent, AudioFault>;` (verifies the existing applied revision and exact context, then emits `ArmApplied` carrying the **provided** generation — never an independently invented counter), `pub fn apply_disarm(&self, generation: SafetyGeneration) -> AudioEvent;`, `pub fn on_audio_event(&self, ev: &AudioEvent) -> bool;` (validates the exact `SessionContext` + nonce tuple).
  - `pub struct OpusWorker;` with `pub fn new() -> Result<Self, EncodeFault>;`, `pub fn encode(&mut self, frame: &StereoFrame, out: &mut EncodedFrame) -> Result<(), EncodeFault>;` (allocation-free; fills `out.context` including generation), `pub fn rtp_timestamp_step(&self) -> u32;` (120 for the initial 2.5 ms mode), `pub fn lookahead_samples(&self) -> u32;`, `pub fn opus_version() -> &'static str;`. Encoder count = one worker per active listener, max 2 for POC.

- [ ] **Step 1: Write the failing DSP tests**

```rust
#[test]
fn gain_linear_matches_10_pow_db_over_20() {
    assert!((gain_linear(-6.0) - 0.501_187_2).abs() < 1e-6);
}

#[test]
fn nonfinite_gain_is_rejected_never_clamped_to_success() {
    assert_eq!(validate_gain(f32::NAN), Err(AudioFault::InvalidSample));
    assert_eq!(validate_gain(f32::INFINITY), Err(AudioFault::InvalidSample));
}

#[test]
fn finite_out_of_range_gain_is_clamped_and_canonicalized() {
    assert_eq!(validate_gain(-70.0), Ok(-60.0));
    assert_eq!(validate_gain(6.0), Ok(0.0));
}

#[test]
fn bus_headroom_constant_is_minus_27_db() {
    assert_eq!(BUS_HEADROOM_DB, -27.0);
}

#[test]
fn mono_source_is_centered_unity_into_both_sides() {
    assert_eq!(centered_coefficients(), [1.0, 1.0]);
}

#[test]
fn default_gain_ramp_is_5ms_240_frames() {
    // exact ramp-state check: a -60->0 dB step moves no more than 240 frames of samples
    let mut e = AudioEngine::new(48_000, &two_channel_map());
    e.register_session(0, ctx_a(), snapshot_gain(-60.0, false));
    let mut produced = 0u32;
    while produced < 480 {
        let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
        let n = e.process_block(0, &capture_block_constant(0.0, 128), &snapshot_gain(0.0, false), &mut out).unwrap();
        produced += n as u32 * CODEC_FRAME_FRAMES;
    }
    assert!(produced >= 480 - CODEC_FRAME_FRAMES);
    // after one 5 ms ramp the target gain is reached
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p host-core audio::tests`
Expected: FAIL to compile with "cannot find function `gain_linear`".

- [ ] **Step 3: Implement the DSP chain and safety worker**

Implement `Σ sources → fixed headroom → master attenuation → stereo-linked limiter → encoder`. `process_block` reads the multichannel `CaptureBlock` and emits up to three 120-frame `StereoFrame`s, carrying a per-session partial frame across calls; a partial frame is discarded (not stale-replayed) when `audioEpoch`/`safetyGeneration` changes. Mono sources are centered with fixed unity coefficients; stereo-linked pairs preserve L/R with no pan. Per-channel ramps are 5 ms = 240 frames at 48 kHz. The limiter is zero-lookahead instantaneous attack with a proposed 50 ms release, explicitly a probe option that does **not** promise hearing transparency, using a shared gain ratio that bounds digital peak at `-6 dBFS`. Invalid samples fault the affected path; source clipping is unrepaired. Deferred: the 0 / 0.5 / 1 ms lookahead comparison is a later gated experiment. The `SafetyWorker` consumes generations supplied by the `ControlActor`; it never assigns an independent counter.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p host-core audio::tests`
Expected: PASS.

- [ ] **Step 5: Write the failing frame-geometry and encoder tests**

| Case | Input to `process_block` | Held after | Frames returned |
| --- | --- | --- | --- |
| A | 128 frames, none held | 8 | 1 |
| B | 128 frames, 8 held | 16 | 1 |
| C | 256 frames, 16 held | 32 | 2 |

```rust
#[test]
fn process_block_geometry_matches_codec_120_frame_boundary() {
    // Case A then B across two calls with the same session; assert held/returned per table
    let mut e = AudioEngine::new(48_000, &two_channel_map());
    e.register_session(0, ctx_a(), snapshot_gain(0.0, false));
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    assert_eq!(e.process_block(0, &capture_block_constant(0.25, 128), &snapshot_gain(0.0, false), &mut out).unwrap(), 1);
    assert_eq!(e.process_block(0, &capture_block_constant(0.25, 128), &snapshot_gain(0.0, false), &mut out).unwrap(), 1);
    assert_eq!(out[0].frame_count, CODEC_FRAME_FRAMES);
    assert_eq!(out[0].context, ctx_a());
}

#[test]
fn process_block_256_returns_two_full_frames() {
    let mut e = AudioEngine::new(48_000, &two_channel_map());
    e.register_session(0, ctx_a(), snapshot_gain(0.0, false));
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    assert_eq!(e.process_block(0, &capture_block_constant(0.25, 256), &snapshot_gain(0.0, false), &mut out).unwrap(), 2);
}
```

- [ ] **Step 6: Run DSP/geometry tests to verify they pass**

Run: `cargo test -p host-core audio::tests`
Expected: PASS.

- [ ] **Step 7: Write the failing encoder tests**

```rust
#[test]
fn opus_initial_mode_is_48000_stereo_lowdelay_120_step() {
    let w = OpusWorker::new().unwrap();
    assert_eq!(w.rtp_timestamp_step(), 120);
    assert_eq!(OpusWorker::opus_version().is_empty(), false);
}

#[test]
fn encode_fills_context_and_bounded_packet_without_alloc() {
    let mut w = OpusWorker::new().unwrap();
    let frame = test_stereo_frame_120();
    let mut out = EncodedFrame::zeroed();
    w.encode(&frame, &mut out).unwrap();
    assert_eq!(out.context, frame.context);
    assert!(out.packet.len > 0 && out.packet.len as usize <= 1275);
}

#[test]
fn five_ms_probe_would_step_240_but_initial_stays_120() {
    assert_eq!(rtp_step_for(RtpFrameMode::Ms2p5), 120);
    assert_eq!(rtp_step_for(RtpFrameMode::Ms5), 240);
}
```

- [ ] **Step 8: Run encoder tests to verify they fail**

Run: `cargo test -p host-core encoder::tests`
Expected: FAIL to compile with "cannot find type `OpusWorker`".

- [ ] **Step 9: Implement the Opus worker**

Configure `opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::LowDelay)`, then call `set_bitrate(opus::Bitrate::Bits(128_000))`, `set_dtx(false)`, `set_inband_fec(false)`, and `set_force_channels(Some(opus::Channels::Stereo))`; read `get_lookahead()`. Encode with `encode_float(&buf, out_bytes)` over a reused fixed output buffer (never `encode_vec`), with 240 interleaved floats for 120 frames. `EncodedFrame` carries `SessionContext` (including generation) so the Task 5 backend gate can check disarm before network submission. Initial RTP step is fixed at 120.

- [ ] **Step 10: Run encoder tests to verify they pass**

Run: `cargo test -p host-core encoder::tests`
Expected: PASS.

- [ ] **Step 11: Run the isolation and safety tests**

```rust
#[test]
fn two_sessions_produce_different_stereo_contributions() {
    let a = render_one_frame(snapshot_gain(-3.0, SourceId::from_bytes([1; 16])));
    let b = render_one_frame(snapshot_gain(-9.0, SourceId::from_bytes([1; 16])));
    assert_ne!(a, b); // different personal mixes from the same input; not an SPL claim
}

#[test]
fn safety_worker_consumes_provided_generation_and_never_invents_one() {
    let w = SafetyWorker::new(sess());
    let ev = w.apply_arm(&listen_arm_with_gen(0), SafetyGeneration(5)).unwrap();
    assert_eq!(ev, AudioEvent::ArmApplied { context: ctx_a(), arm_nonce: listen_arm_with_gen(0).arm_nonce });
}

#[test]
fn output_never_exceeds_minus_6_dbfs_ceiling() {
    let limit = 10f32.powf(-6.0 / 20.0);
    let out = render_unity_correlated();
    assert!(out.iter().all(|s| s.abs() <= limit + 1e-6));
}

#[test]
fn invalid_sample_faults_the_affected_path() {
    assert_eq!(validate_gain(f32::NAN), Err(AudioFault::InvalidSample));
}
```

Run: `cargo test -p host-core audio::tests`
Expected: PASS.
Run: `cargo clippy -p host-core --all-targets -- -D warnings`
Expected: PASS.

### Task 4: HTTPS, pairing, authoritative control, and safety races

**Files:**
- Create: `crates/host-core/src/server/tls.rs`, `server/pairing.rs`, `server/http.rs`, `server/ws.rs`
- Create: `crates/host-core/src/control/auth.rs`, `control/revisions.rs`, `control/actor.rs`
- Modify (Lane B only): `crates/host-core/src/server/mod.rs`, `src/control/mod.rs`, `src/config/mod.rs`
- Test: `crates/host-core/src/server/tests.rs`, `crates/host-core/src/control/tests.rs`

**Interfaces:**
- Consumes: all wire DTOs, `SessionContext`, `AudioEvent`, `Clock`, `Entropy`, `DspControl`, `AudioEventSink`, `ControlError`, `SafetyGeneration`, `PairingCredential` (Task 1); `SafetyWorker` semantics (Task 3, conceptual).
- Produces: `pub struct ControlActor;` with `pub fn new(clock: Arc<dyn Clock>, dsp: Arc<dyn DspControl>, catalog: CatalogSnapshot, host_epoch: HostEpoch, audio_epoch: AudioEpoch) -> Self;`, `pub fn current_generation(&self, session: SessionEpoch) -> SafetyGeneration;`, `pub fn apply_patch(&mut self, session: SessionEpoch, request: MixPatch) -> Result<MixAck, ControlError>;`, `pub fn arm(&mut self, session: SessionEpoch, request: ListenArm) -> Result<(), ControlError>;`, `pub fn disarm(&mut self, session: SessionEpoch, request: ListenDisarm) -> Result<(), ControlError>;`, `pub fn on_audio_event(&mut self, ev: AudioEvent) -> Option<ServerMessage>;` (validates typed context before emitting `mix.applied`). `disarm` calls `DspControl::disarm(session, new_generation)` (priority) so the shared gate closes atomically. `pub fn issue_pairing_credential(&mut self) -> PairingCredential;` (fresh single-use, above the operator IPC). HTTP routes: `GET /join` (static musician page only), `POST /api/v1/pair`, `GET /api/v1/ws` (WSS). In-memory session store with injected `Clock`/`Entropy`; no token persistence. `ControlError` includes a `TlsIdentityInvalid` variant distinct from the other structured errors.

- [ ] **Step 1: Write the failing pairing/auth tests**

```rust
#[test]
fn pairing_token_is_256bit_single_use_and_expires_at_120s() {
    let mut store = PairStore::new(test_entropy(), test_clock());
    let t = store.issue();
    assert!(store.exchange(&t).is_ok());
    assert!(store.exchange(&t).is_err()); // one use
    let t2 = store.issue();
    test_clock().advance_secs(121);
    assert!(store.exchange(&t2).is_err());
}

#[test]
fn pairing_credentials_are_never_logged() {
    let sink = test_log_sink();
    let t = PairStore::new(test_entropy(), test_clock()).issue();
    assert!(!sink.contains(&t));
}

#[test]
fn issue_pairing_credential_returns_join_url_and_expiry() {
    let mut a = control_actor();
    let c = a.issue_pairing_credential();
    assert!(c.join_url.starts_with("https://"));
    assert_eq!(c.expires_in_seconds, 120);
}

#[test]
fn ws_requires_strict_origin_before_upgrade() {
    assert_eq!(origin_gate(state(), "https://host.local:8443"), 101);
    assert_eq!(origin_gate(state(), "https://evil.example"), 403);
}

#[test]
fn session_cookie_is_secure_httponly_samesite_strict() {
    let c = issue_cookie(test_clock());
    assert!(c.contains("Secure") && c.contains("HttpOnly") && c.contains("SameSite=Strict"));
}

#[test]
fn unmute_is_forbidden_while_unarmed_but_gain_is_allowed() {
    let mut a = control_actor();
    assert!(matches!(a.apply_patch(sess(), patch_unmute_channel()), Err(ControlError::UnarmedUnmuteForbidden)));
    assert!(a.apply_patch(sess(), patch_gain_only()).is_ok());
}

#[test]
fn mute_is_always_allowed_even_when_unarmed() {
    let mut a = control_actor();
    assert!(a.apply_patch(sess(), patch_mute_channel()).is_ok());
}

#[test]
fn two_musicians_can_listen_to_the_same_source_with_independent_settings() {
    // authenticated at the server boundary via the real request handler, not by calling apply_patch for another session
    let mut server = test_server_with_two_pairings(); // cookieA/sessionA, cookieB/sessionB
    assert_eq!(server.post_mix("cookieA", patch_source_gain(shared_source(), -3.0)).status(), 200);
    assert_eq!(server.post_mix("cookieB", patch_source_gain(shared_source(), -9.0)).status(), 200);
    assert_eq!(server.accepted_gain("cookieA", shared_source()), -3.0); // check ACCEPTED canonical state
    assert_eq!(server.accepted_gain("cookieB", shared_source()), -9.0);
}

#[test]
fn unavailable_source_is_forbidden_within_logical_permissions() {
    let mut a = control_actor();
    assert!(matches!(a.apply_patch(sess(), patch_source_gain(unavailable_source(), -3.0)),
        Err(ControlError::SourceForbidden)));
}

#[test]
fn cross_mix_session_id_is_rejected_at_the_server_boundary() {
    // cookieA presenting envelope sessionEpoch = sessionB must be rejected
    let mut server = test_server_with_two_pairings();
    assert_eq!(server.post_mix_with_session("cookieA", session_b(), patch_gain_only()).status(), 403);
}
```

- [ ] **Step 2: Run pairing/auth tests to verify they fail**

Run: `cargo test -p host-core server::tests`
Expected: FAIL to compile with "cannot find function `control_actor`".

- [ ] **Step 3: Write the failing safety-race tests**

```rust
#[test]
fn stale_generation_disarm_for_authenticated_current_session_is_applied() {
    let mut a = control_actor();
    let _ = a.apply_patch(sess(), patch_gain_only());
    let _ = a.arm(sess(), listen_arm_with_gen(0));
    assert_eq!(a.current_generation(sess()), SafetyGeneration(1)); // arm assigned gen 1
    let stale = ListenDisarm { session_epoch: sess(), safety_generation: SafetyGeneration(0) }; // stale gen 0
    assert!(a.disarm(sess(), stale).is_ok());                      // still accepted for current session
    assert_eq!(a.current_generation(sess()), SafetyGeneration(2)); // disarm bumped to 2
}

#[test]
fn stale_worker_cannot_arm_after_disarm() {
    let mut a = control_actor();
    let _ = a.arm(sess(), listen_arm_with_gen(0));
    let _ = a.disarm(sess(), listen_disarm(a.current_generation(sess())));
    assert!(a.on_audio_event(ArmApplied { context: ctx_a(), arm_nonce: old_nonce() }).is_none());
}

#[test]
fn cancelled_arm_nonce_is_permanently_invalid_for_that_attempt() {
    let mut a = control_actor();
    let n = ArmNonce::from_bytes([5; 16]);
    let _ = a.arm(sess(), listen_arm_with_nonce(n));
    let _ = a.disarm(sess(), listen_disarm(a.current_generation(sess())));
    assert!(a.on_audio_event(ArmApplied { context: ctx_a(), arm_nonce: n }).is_none());
}

#[test]
fn mix_applied_is_emitted_only_for_installed_revision() {
    let mut a = control_actor();
    let ack = a.apply_patch(sess(), patch_gain_only()).unwrap();
    assert!(a.on_audio_event(MixApplied { applied_revision: ack.accepted_revision, context: ctx_a(), start_sample: 0, snapshot: ack.canonical_snapshot }).is_some());
    assert!(a.on_audio_event(MixApplied { applied_revision: MixRevision(9999), context: ctx_a(), start_sample: 0, snapshot: snapshot_gain(-3.0, shared_source()) }).is_none());
}
```

- [ ] **Step 4: Run control tests to verify they fail**

Run: `cargo test -p host-core control::tests`
Expected: FAIL to compile with "cannot find type `ControlActor`".

- [ ] **Step 5: Implement the server and control actor**

Implement `axum` 0.8.9 (`ws` feature) plus `axum-server` 0.8.0 `tls-rustls` (indirect rustls 0.23 / tokio-rustls 0.26, avoiding redundant TLS dependencies). Pairing: OS-random 256-bit, one-use, 120 s token in a QR fragment, exchanged via `POST`, removed from history, never logged; the operator path is `issue_pairing_credential` (fresh per phone). Sessions: `Secure`, `HttpOnly`, `SameSite=Strict` cookie with an exact allowed-origins list; draft 12 h expiry; one active receiver per musician (replacement disarms the old); POC cap 2 (production 8 is not the POC bound); bounded pair sessions and control rates. `arm` carries the current epochs, `safetyGeneration`, `applied_revision`, and a fresh `armNonce`; the host serializes arms, assigns a new generation, and confirms the exact tuple before `listen.armed`. `disarm` increments the generation separately and is priority-valid for the authenticated current session, so a stale worker cannot arm. `mix.applied` is emitted only for installed snapshots; skipped/coalesced revisions are never confirmed. Only one patch in flight; unsent edits coalesce preserving newer intent; rate limit 30 patch/s with bounded burst. **Authorization is enforced at the server boundary** (cookie + envelope session), not by trusting a caller-passed session.

- [ ] **Step 6: Run server and control suites separately to verify they pass**

Run: `cargo test -p host-core server::tests`
Expected: PASS.
Run: `cargo test -p host-core control::tests`
Expected: PASS.
Run: `cargo clippy -p host-core --all-targets -- -D warnings`
Expected: PASS.

### Task 5: str0m media transport, mDNS adapter, and backend safety gate

**Files:**
- Create: `crates/host-core/src/transport/str0m_session.rs`, `transport/sdp.rs`, `transport/mdns.rs`, `transport/safety_gate.rs`
- Modify (Lane C only): `crates/host-core/src/transport/mod.rs`
- Test: `crates/host-core/src/transport/tests.rs`

**Interfaces:**
- Consumes: `MediaCommand`, `MediaEvent`, `EncodedFrame`, `SessionContext`, `SessionEpoch`, `MediaFailureCode` (Task 1).
- Produces: `pub struct MediaSession` (uses the Task 1 `InterfaceInfo` as its selected-interface type — no new duplicate struct) with `pub fn new(session: SessionEpoch, iface: InterfaceInfo) -> Result<Self, MediaFailureCode>;`, `pub fn handle_offer(&mut self, sdp: &str) -> Result<String, MediaFailureCode>;`, `pub fn add_candidate(&mut self, candidate: Option<&str>) -> Result<(), MediaFailureCode>;`, `pub fn poll_event(&mut self) -> Option<MediaEvent>;`, `pub fn write_packet(&mut self, frame: &EncodedFrame) -> Result<(), MediaFailureCode>;`. The browser offer creates one recvonly audio transceiver; the answer is sendonly; exactly one intended audio section is allowed and extra media is rejected.

- [ ] **Step 1: Write the failing SDP/transport tests**

```rust
#[test]
fn offer_is_recvonly_audio_only_and_answer_is_sendonly() {
    let mut s = media_session();
    let answer = s.handle_offer(recvonly_audio_offer()).unwrap();
    assert!(answer.contains("a=sendonly"));
}

#[test]
fn extra_media_section_is_rejected() {
    let mut s = media_session();
    assert_eq!(s.handle_offer(offer_with_video()), Err(MediaFailureCode::ExtraMediaRejected));
}

#[test]
fn fmtp_adaptation_is_pure_and_retains_a_nondefault_payload_type() {
    let (_, pt) = adapt_fmtp(offer_with_pt(123));
    assert_eq!(pt, 123); // retains the actual PT; forcing/echoing 111 proves nothing
    assert_eq!(adapt_fmtp(offer_with_pt(123)).1, 123);
}

#[test]
fn mono_fmtp_is_explicitly_refused() {
    assert_eq!(negotiate_stereo(offer_mono_fmtp()), Err(MediaFailureCode::NegotiationFailed));
}

#[test]
fn valid_selected_lan_ip_candidate_is_accepted_without_resolver_call() {
    let mut mdns = MdnsAdapter::new_recording_resolver(); // records whether resolver was called
    assert!(mdns.add(Some("candidate:1 1 udp 1 192.168.1.5 5000 typ host")).is_ok());
    assert!(!mdns.resolver_called());
}

#[test]
fn dot_local_candidate_uses_bounded_resolver_before_parse() {
    let mut mdns = MdnsAdapter::new_resolver(["10.0.0.9:5000"]);
    assert!(mdns.add(Some("candidate:1 1 udp 1 3f2a.local 5000 typ host")).is_ok());
    assert!(mdns.resolver_called());
}

#[test]
fn mdn_resolution_failure_is_visible_and_escalates() {
    let mut mdns = MdnsAdapter::new_failing_resolver(); // injected resolver failure
    assert_eq!(mdns.add(Some("candidate:1 1 udp 1 3f2a.local 5000 typ host")), Err(MediaFailureCode::MdnUnresolved));
}

#[test]
fn none_candidate_signals_end_of_trickle_without_error() {
    assert!(add_candidate(None).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p host-core transport::tests`
Expected: FAIL to compile with "cannot find type `MediaSession`".

- [ ] **Step 3: Implement the transport and backend safety gate**

Use str0m 0.24.1 with `default-features = false` and `aws-lc-rs` enabled. Configure explicit codec params for stereo with DTX/FEC off, without a contradictory `minptime`; do not enable the full default Opus (minptime 10 / FEC) because it conflicts with 2.5 ms frames. Source the actual `Writer.write` / `accept_offer` / `MediaTime` usage from str0m docs during implementation; if a type is ambiguous, read raw str0m docs — do not invent a `MediaTime` SDK factory. After every mutation, drain `poll_output` until `OutputTimeout`, then `send_transmit`/`feed_input`/`receive` on a deadline: single-owner RTC event loop, one socket per listener, no shared Rust access across threads; handle queued catch-up bursts and `None`/null end-of-candidates. The **backend safety gate must check the `EncodedFrame` `SessionContext` (including generation) before submitting network packets**, holds a bounded stack buffer, and must not retain a stale queue after disarm. mDNS: a **valid selected-LAN IP candidate is accepted with no resolver call**; only `.local` hostnames are resolved (best-effort, not a Bonjour guarantee) by a bounded adapter with an injected resolver, before further parsing, rate- and count-limited to selected LAN hosts. Only an injected resolver failure maps to `MdnUnresolved`. Failure is escalated, never worked around with plaintext or insecure global browser flags.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p host-core transport::tests`
Expected: PASS.
Run: `cargo clippy -p host-core --all-targets -- -D warnings`
Expected: PASS.

### Task 6: Browser receiver lifecycle and diagnostics

**Files:**
- Create: `apps/web/src/transport/ReceiverController.ts`, `apps/web/src/transport/diagnostics.ts`, `apps/web/src/transport/__tests__/controller.test.ts`, `apps/web/src/transport/__tests__/diagnostics.test.ts`
- Create: `apps/web/src/transport/__tests__/helpers.ts` (mock bridge/session helpers used by the tests here)

**Interfaces:**
- Consumes: frozen `createReceiverController`, `ReceiverPorts` (`MusicianSignaling`, `AudioMediaPort`, `ActivationPort`, `StatsPort`), `ReceiverSnapshot`, `MixPatch`, `MixAck`, `DiagnosticsSnapshot` interfaces (Task 1). **It must never import `HostBridge` or `@tauri-apps/api`** — the LAN musician bundle has no privilege.
- Produces: the `ReceiverController` implementation with a **sticky local safety gate**; `computeNetworkRttMs`, `computeBufferDelayMs`, and `DiagnosticsSnapshot` with `end_to_end: "Not measured"`. WebRTC setup creates exactly one recvonly audio transceiver (no mic permission, no video, no datachannel). `requestMix` resolves on **accepted**, not DSP-applied; the snapshot updates `applied_mix` only when a `mix.applied` event arrives.

- [ ] **Step 1: Write the failing lifecycle tests**

```ts
import { createReceiverController } from '../ReceiverController';
import { fakePorts, flushEvents } from './helpers'; // fakes implement AudioMediaPort/MusicianSignaling/ActivationPort/StatsPort

test('arming_requires_explicit_gesture_and_playback_readiness', async () => {
  const ports = fakePorts({ gesture: false });
  const c = createReceiverController(ports);
  await expect(c.arm()).rejects.toThrow(/gesture/);
});

test('arm_invokes_media_play_from_gesture_before_awaiting_host', async () => {
  const ports = fakePorts({ gesture: true });
  const c = createReceiverController(ports);
  const p = c.arm();                       // must call play() synchronously from the gesture
  expect(ports.media.playCalls).toBe(1);
  await p;
});

test('mute_true_is_always_local_immediate_even_when_unarmed_disconnected', () => {
  const ports = fakePorts({ armed: false, connected: false });
  const c = createReceiverController(ports);
  c.personalMasterMute(true);
  expect(ports.media.muted).toBe(true);          // assert the audio output port, not a phase change
  expect(c.getSnapshot().master_local_muted).toBe(true);
});

test('unmute_false_requires_armed_current_readiness_and_gesture', () => {
  const ports = fakePorts({ armed: false, ready: false });
  const c = createReceiverController(ports);
  c.personalMasterMute(false);
  expect(ports.media.muted).toBe(true);          // stays muted without armed+readiness
});

test('stop_is_immediate_without_ack_and_invalidates_nonce', () => {
  const ports = fakePorts({ armed: true });
  const c = createReceiverController(ports);
  c.stop();
  expect(ports.media.stopCalls).toBe(1);
  expect(c.getSnapshot().phase).toBe('ready-muted');
});

test('request_mix_resolves_on_accepted_not_on_dsp_applied', async () => {
  const ports = fakePorts({ acceptedRevision: '12' });
  const c = createReceiverController(ports);
  const ack = await c.requestMix(patchGain(-9));
  expect(ack.acceptedRevision).toBe('12');       // camelCase wire, string counter
  expect(BigInt(ack.acceptedRevision)).toBe(12n);
  expect(c.getSnapshot().applied_mix).toBeNull();
});

test('stale_arm_nonce_reply_is_ignored', async () => {
  const ports = fakePorts();
  const c = createReceiverController(ports);
  ports.signaling.emitServerEvent({ type: 'listen.armed', armNonce: '44444444-4444-4444-8444-444444444444' }); // cancelled nonce; helper fills the current envelope/context
  await flushEvents();
  expect(c.getSnapshot().phase).not.toBe('armed');
});

test('acknowledgement_never_opens_sticky_gate', async () => {
  const ports = fakePorts();
  const c = createReceiverController(ports);
  ports.signaling.emitServerEvent({ type: 'mix.applied', appliedRevision: '12' });
  await flushEvents();
  expect(c.getSnapshot().master_local_muted).toBe(true);
});
```

- [ ] **Step 2: Run lifecycle tests to verify they fail**

Run: `npm run test --workspace apps/web -- transport/__tests__/controller`
Expected: FAIL with "Cannot find module '../ReceiverController'".

- [ ] **Step 3: Implement the receiver controller**

Implement the sticky local gate over the injected ports. `personalMasterMute(true)` is always a local immediate mute (armed, disconnected, no activation all allowed). Only `personalMasterMute(false)` requires an explicit gesture, an armed session with the **current** generation, and playback readiness. `arm()` calls `ports.media.play()` from the gesture **before** awaiting host confirmation, and the local gate opens only when both the host confirmation and browser playback readiness hold (both are required; progress statistics are not proof of audibility). Acknowledgements, preset recovery, reconnects, and transport recovery never reopen the gate; a cancelled `armNonce` is permanently invalid for that attempt. Stop silences locally with no network-acknowledgement wait.

- [ ] **Step 4: Run lifecycle tests to verify they pass**

Run: `npm run test --workspace apps/web -- transport/__tests__/controller`
Expected: PASS.

- [ ] **Step 5: Write the failing diagnostics tests**

```ts
import { computeNetworkRttMs, computeBufferDelayMs } from '../diagnostics';

test('diagnostics_show_unavailable_never_zero', () => {
  expect(computeNetworkRttMs(undefined)).toBe('Unavailable');
  expect(computeNetworkRttMs(undefined)).not.toBe(0);
});

test('network_rtt_uses_selected_pair_currentRoundTripTime_times_1000', () => {
  expect(computeNetworkRttMs({ currentRoundTripTime: 0.042 })).toBe(42);
});

test('buffer_delay_first_sample_shows_waiting_for_sample', () => {
  const first = { sessionEpoch: 's', sampledAtMs: 1000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 100, delay: 1.0 };
  expect(computeBufferDelayMs(null, first)).toBe('Waiting for sample');
});

test('buffer_delay_zero_emitted_delta_is_unavailable', () => {
  const prev = { sessionEpoch: 's', sampledAtMs: 1000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 100, delay: 1.0 };
  const same = { sessionEpoch: 's', sampledAtMs: 2000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 100, delay: 1.0 };
  const v = computeBufferDelayMs(prev, same);
  expect(v).toBe('Unavailable');              // no emitted samples; never fabricate zero delay
});

test('buffer_delay_changed_ids_resets_to_waiting_for_sample', () => {
  const prev = { sessionEpoch: 's', sampledAtMs: 1000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 100, delay: 1.0 };
  const moved = { sessionEpoch: 's', sampledAtMs: 2000, statsId: 'b', stream: 'audio', epoch: 'e', emitted: 200, delay: 1.2 };
  expect(computeBufferDelayMs(prev, moved)).toBe('Waiting for sample');
});

test('buffer_delay_valid_interval_computes_ms', () => {
  const prev = { sessionEpoch: 's', sampledAtMs: 1000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 100, delay: 1.0 };
  const good = { sessionEpoch: 's', sampledAtMs: 2000, statsId: 'a', stream: 'audio', epoch: 'e', emitted: 200, delay: 1.2 };
  expect(computeBufferDelayMs(prev, good)).toBeCloseTo(2.0, 5); // 1000 * 0.2 / 100
});

test('diagnostics_never_halve_rtt_and_never_sum_rtt_plus_buffer', () => {
  const snap = buildDiagnostics(42, 2);
  expect(snap.end_to_end).toBe('Not measured');
  expect(Object.prototype.hasOwnProperty.call(snap, 'oneWay')).toBe(false);
  expect(Object.prototype.hasOwnProperty.call(snap, 'total')).toBe(false);
});
```

- [ ] **Step 6: Run diagnostics tests to verify they fail**

Run: `npm run test --workspace apps/web -- transport/__tests__/diagnostics`
Expected: FAIL with "Cannot find module '../diagnostics'".

- [ ] **Step 7: Implement diagnostics**

`computeNetworkRttMs` reads the selected candidate pair as `currentRoundTripTime * 1000` (seconds to ms). `computeBufferDelayMs` uses `1000 * Δ(jitterBufferDelay) / Δ(jitterBufferEmittedCount)` only when the same session, stats ID, and stream hold in a consistent epoch, with a positive emitted count and finite, non-negative deltas. A **first sample or a changed stats ID/session/epoch** resets to `"Waiting for sample"`; zero emitted delta, missing, stale, or unsupported data yields `"Unavailable"`, never fabricated zero. Refresh once per second, independent of the 10 Hz meters and of the 500 ms watchdog / 3-miss heartbeat. The getStats timestamp alone is not proof of a new RTT sample. End-to-end stays `"Not measured"`; never halve RTT, never sum RTT + buffer. Diagnostics fixtures include the declared `sessionEpoch` and `sampledAtMs`; test a changed session separately from a changed audio epoch.

- [ ] **Step 8: Run diagnostics tests to verify they pass**

Run: `npm run test --workspace apps/web -- transport/__tests__/diagnostics`
Expected: PASS.
Run: `npm run typecheck --workspace apps/web`
Expected: PASS.

### Task 7: Tailwind primitives, mobile views, admin views (designer-owned)

**Files:**
- Create: `apps/web/src/styles/tokens.css`, `apps/web/src/ui/Button.tsx`, `ui/GainControl.tsx`, `ui/ChannelStrip.tsx`, `ui/DigitalMeter.tsx`, `ui/StatusBadge.tsx`, `ui/Notice.tsx`, `ui/LatencyDiagnostics.tsx`
- Modify (Lane D only): `apps/web/src/admin/AdminRoot.tsx`, `apps/web/src/musician/MusicianRoot.tsx` (replace the Task 1 stubs)
- Create: `apps/web/src/musician/JoinView.tsx`, `musician/ReceiverView.tsx`, `apps/web/src/admin/SourcesView.tsx`, `admin/DeviceView.tsx`, `admin/PairingView.tsx`
- Test: `apps/web/src/ui/__tests__/*.test.tsx`, `apps/web/src/musician/__tests__/*.test.tsx`, `apps/web/src/admin/__tests__/*.test.tsx`

**Interfaces:**
- Consumes: frozen `HostBridge` + `ReceiverController` interfaces and the `PairingCredential` type (Task 1) and **mock controllers only**; `qrcode.react` 4.2.0 (peer React 19) for the QR image (bundled at build time, no runtime network); design tokens from DESIGN-SYSTEM §2/§4 and its `LatencyDiagnostics` subsection.
- Declared component props (short contract, so tests and components agree): `GainControlProps { label: string; valueDb: number; minDb?: number; maxDb?: number; muted?: boolean; onChange(db: number): void; }` (defaults `minDb = -60`, `maxDb = 0`); `ChannelStripState { sourceId: string; label: string; requestedDb: number; appliedDb: number; muted: boolean }`; `ChannelStripProps { state: ChannelStripState; onGainChange(db: number): void; onMuteChange(muted: boolean): void }`; `DigitalMeterProps { value: number | undefined }`; `LatencyDiagnosticsProps { snapshot: DiagnosticsSnapshot }`; `PairingView` props `{ bridge: HostBridge }`. `mockStrip` is a test fixture returning `ChannelStripState`, not a mock-only production prop.
- Produces: DOM/semantic behavior in RTL. **Mutable widget UI (GainControl, ChannelStrip, Master dock) is a critical designer deliverable; the builder/orchestrator does not implement it.** These tests cover semantics and behavior, not pixel layout; no hardware-complete claim. Lane D owns its own `./helpers.ts` test helpers and all UI files; no root manifest edit.

- [ ] **Step 1: Write the failing accessibility/behavior tests**

```tsx
import { render, screen, fireEvent } from '@testing-library/react';
import { GainControl } from '../GainControl';
import { ChannelStrip } from '../ChannelStrip';
import { DigitalMeter } from '../DigitalMeter';
import { LatencyDiagnostics } from '../LatencyDiagnostics';
import { PairingView } from '../../admin/PairingView';
import { mockStrip, diagSnapshot, fakeBridge } from './helpers';

test('gain_control_uses_named_native_range_with_aria_valuetext', () => {
  render(<GainControl label="Lead vocal" valueDb={-12} onChange={() => {}} />);
  const range = screen.getByRole('slider', { name: /lead vocal/i });
  expect(range).toHaveAttribute('aria-valuetext', expect.stringMatching(/minus 12 decibels/i));
});

test('channel_mute_shows_requested_pending_until_host_applied_revision', () => {
  render(<ChannelStrip state={mockStrip({ requestedDb: -12, appliedDb: -6 })} onGainChange={() => {}} onMuteChange={() => {}} />);
  expect(screen.getByText(/requested -12 db .* pending/i)).toBeInTheDocument();
  expect(screen.getByText(/applied -6 db/i)).toBeInTheDocument();
});

test('missing_telemetry_renders_unavailable_never_zero', () => {
  render(<DigitalMeter value={undefined} />);
  expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
  expect(screen.queryByText('0')).not.toBeInTheDocument();
});

test('mute_is_labeled_logical_neg_infinity_separately_from_lowest_finite_gain', () => {
  render(<GainControl label="Keys" valueDb={-60} muted onChange={() => {}} />);
  expect(screen.getByText(/muted \(-\u221e db\)/i)).toBeInTheDocument();
});

test('latency_diagnostics_show_not_measured_and_stacked_rows', () => {
  render(<LatencyDiagnostics snapshot={diagSnapshot({ rtt: 42, buffer: 2 })} />);
  expect(screen.getByText(/network rtt/i)).toBeInTheDocument();
  expect(screen.getByText(/^not measured$/i)).toBeInTheDocument();
});

test('pairing_view_issues_a_fresh_single_use_credential_and_never_logs_it', async () => {
  const bridge = fakeBridge({ joinUrl: 'https://host.local:8443/join#t=SECRET', expiresInSeconds: 120 });
  render(<PairingView bridge={bridge} />);
  fireEvent.click(screen.getByRole('button', { name: /generate pairing/i }));
  expect(await screen.findByText(/expires in 120 s/i)).toBeInTheDocument();
  expect(bridge.issueCalls).toBe(1);            // fresh credential per phone
  expect(bridge.logged).not.toContain('SECRET'); // private token never written to logs
});
```

- [ ] **Step 2: Run semantics tests to verify they fail**

Run: `npm run test --workspace apps/web -- ui`
Expected: FAIL with "Cannot find module '../GainControl'".

- [ ] **Step 3: Implement primitives and views against mocks**

Use Tailwind generated and bundled at build time with local assets, fed by shared semantic tokens; no raw hex in components. Labels: mute is `"Muted (−∞ dB)"`; requested vs applied are distinct; the LatencyDiagnostics rows use `Unavailable` / `Waiting for sample` / `Not measured` exactly. The Master dock keeps an immediate silence action. Replace the Task 1 `AdminRoot`/`MusicianRoot` stubs with the real roots.

- [ ] **Step 4: Run semantics tests to verify they pass**

Run: `npm run test --workspace apps/web -- ui`
Expected: PASS.
Run: `npm run test --workspace apps/web -- musician admin`
Expected: PASS.
Run: `npm run typecheck --workspace apps/web`
Expected: PASS.

- [ ] **Step 5: Note the layout deferral (do not assert pixels in jsdom)**

Layout behaviors jsdom cannot prove (320px horizontal overflow, 200% zoom, dark/light rendering, forced-colors, reduced-motion) are asserted in Playwright in Task 9, **not** here. Task 7 asserts DOM semantics only. Tailwind 4 requires **Safari 16.4+ / Chrome 111+** — confirm the actual test devices in Task 10; emulated WebKit is not iPhone qualification.

### Task 8: Desktop composition, Tauri permissions, asset embedding, and early mDNS interop probe

**Files:**
- Create: `apps/desktop/src-tauri/src/commands.rs`, `src/asset_embed.rs`, `src/window_guard.rs`, `src/probe.rs`
- Create: `apps/desktop/src-tauri/permissions/operator.toml`, `apps/desktop/src-tauri/Info.plist`, `apps/desktop/src-tauri/Entitlements.plist`
- Create: `apps/desktop/src-tauri/build.rs` (custom-command manifest wiring)
- Modify (INT only): `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/capabilities/main.json`, `src/main.rs`, `src/lib.rs`
- Test: `apps/desktop/src-tauri/tests/composition.rs`, `apps/desktop/src-tauri/tests/window_guard.rs`

**Interfaces:**
- Consumes: `HostBridge`, `AssetProvider`, `InterfaceInfo` (as selected interface), `MediaSession`, `ControlActor`, `CaptureService` (Tasks 1–6).
- Produces: an injected `AssetProvider` so `cargo test -p host-core` needs no `frontendDist`; Tauri composes one host process and exposes only allowlisted commands in a **custom command permission manifest** plus registration, not capability alone; a registration entry for `issue_pairing_credential`.

- [ ] **Step 1: Write the failing composition and guard tests**

```rust
#[test]
fn asset_provider_trait_allows_host_core_tests_without_frontend_dist() {
    let p: Box<dyn AssetProvider> = Box::new(FixtureAssets);
    assert!(p.get("/join").is_some());
}

#[test]
fn ipc_command_rejects_non_operator_window_label() {
    assert!(authorize_window("operator").is_ok());
    assert!(authorize_window("other").is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p iem-cast-desktop --test composition`
Expected: FAIL to compile with "cannot find type `AssetProvider`" or missing command registration.

- [ ] **Step 3: Build the web assets first**

Run: `npm run build:web`
Expected: PASS, producing `apps/web/dist/admin` and `apps/web/dist/musician`. (The native composition test needs these; build them **before** any Tauri build/test so a failure is meaningful and not an unrelated missing-asset error.)

- [ ] **Step 4: Compose the desktop shell, permissions, and macOS privacy keys**

Tauri config: `devUrl` = `http://127.0.0.1:5173` (IPv4 only, matching Vite), `frontendDist` = `../../web/dist/admin` relative to `apps/desktop/src-tauri`, **no `beforeBuildCommand` hook** (the root `build:web` script owns ordering, since the Tauri config cwd is not the repo root). Shell-owned rust-embed covers the musician frontend dir only (`../../web/dist/musician`, `debug-embed` in dev) and fails the desktop build when missing.

Register custom commands so they are **not** exposed to all windows by default: in `build.rs`, call `tauri_build::try_build(Attributes::new().app_manifest(AppManifest::new().commands(&["list_devices", "list_interfaces", "start_host", "stop_host", "source_catalog", "set_available_sources", "set_source_label", "issue_pairing_credential"])))`. Define the matching custom permission in `permissions/operator.toml` as `[[permission]] identifier = "operator-control"` with `commands.allow = [<same names>]`; the capability has an operator-only scope and grants no remote-URL. Validate the caller window label/origin to the exact bundled operator window only (dev: loopback only); reject LAN and other windows. **The exact caller-check API is not verified by the plan — verify it against the primary Tauri source during implementation, do not guess.** Add no musician filesystem/shell/root-trust command. `tauri-build` 2.7.1 is a separate build dependency from runtime Tauri 2.12.1; keep both in the baseline.

macOS privacy (per Apple/Tauri primary sources): Tauri merges `src-tauri/Info.plist`, so add `NSMicrophoneUsageDescription` with the exact copy `IEM Cast captures USB audio to create personal monitor mixes.` — a microphone usage string is required for the mic APIs and there is **no documented USB exemption**. For hardened-runtime signing include `Entitlements.plist` with the boolean `com.apple.security.device.audio-input = true`, and set `bundle.macOS.entitlements = "./Entitlements.plist"`. **No App Sandbox is needed.** Confirm the packaged app can actually start CoreAudio before claiming it works.

- [ ] **Step 5: Run the desktop composition gate after assets exist**

Run: `cargo build -p iem-cast-desktop`
Expected: PASS.
Run: `cargo test -p iem-cast-desktop --test composition`
Expected: PASS.
Run: `cargo test -p iem-cast-desktop --test window_guard`
Expected: PASS.

- [ ] **Step 6: Run the early mDNS interop probe (diagnostic mode, before full shell polish; may be blocked)**

Run: `cargo run -p iem-cast-desktop -- probe --diagnostic-audio`
Expected: using the `probe.rs` diagnostic composition entry (explicit diagnostic mode; **not** a claim that the real Soundcraft is working), a real browser on the LAN resolves `.local` candidates and negotiates a single stereo audio section. **Reuse the ordinary `JoinView`/`ReceiverView`** from Task 7 — do not add a separate "no receiver UI" diagnostic view outside the owner graph. Before this step, the operator must have trusted the local root CA on the test device (a **manual operator action** that may occur before Task 10). If mDNS cannot resolve, **escalate** — do not continue with plaintext or insecure global browser flags. This probe is optional and, if hardware/trust is unavailable, it is recorded as **blocked** and **does not prevent the automated Task 9 gate**.

### Task 9: Complete automated integration/browser gate

**Files:**
- Create (INT): `crates/host-core/tests/poc_flow.rs`, `playwright.config.ts`, `apps/web/e2e/poc.spec.ts`, `apps/web/e2e/fixtures/fakeHost.ts`
- Modify (INT only): `apps/web/vitest.config.ts` (exclude `e2e/**`)
- Test: `crates/host-core/tests/poc_flow.rs`, `apps/web/e2e/poc.spec.ts`

**Interfaces:**
- Consumes: Tasks 1–8 and built web assets.
- Produces: an end-to-end **core flow** test over the real control/DSP/Opus/media-adapter composition with only capture/network mocked, plus the browser gate. It runs **before** any manual hardware qualification and is explicitly **not** fulfillment of the real wireless requirement. If physical hardware is absent or fails, **Task 10 is blocked; it never blocks this software gate or the reporting of software completion.**

- [ ] **Step 1: Write the failing core-flow integration test**

```rust
// crates/host-core/tests/poc_flow.rs — real control + DSP + Opus + media adapter; capture and network are mocks only
#[test]
fn two_sessions_apply_arm_and_stop_generations_without_hardware() {
    let (mut server, capture) = composed_host_with_fake_capture();
    let a = server.pair_and_session("cookieA");
    let b = server.pair_and_session("cookieB");
    assert_eq!(server.post_mix(&a, patch_source_gain(shared_source(), -3.0)).status(), 200);
    assert_eq!(server.post_mix(&b, patch_source_gain(shared_source(), -9.0)).status(), 200);
    assert_eq!(server.accepted_gain(&a, shared_source()), -3.0);
    assert_eq!(server.accepted_gain(&b, shared_source()), -9.0);
    capture.push_frames(256);
    server.arm(&a, listen_arm_with_gen(0)).unwrap();
    assert_eq!(server.current_generation(&a), SafetyGeneration(1));
    server.disarm_with_stale_generation(&a, SafetyGeneration(0));
    assert_eq!(server.current_generation(&a), SafetyGeneration(2)); // stale disarm still applies
    // a stale worker ArmApplied at gen 1 must not reopen
    assert!(server.inject_arm_applied(&a, /*gen*/ 1).is_none());
}
```

- [ ] **Step 2: Run the core-flow test to verify it fails**

Run: `cargo test -p host-core --test poc_flow`
Expected: FAIL to compile until `composed_host_with_fake_capture()` exists.

- [ ] **Step 3: Write the failing E2E spec and fake host**

```ts
// apps/web/e2e/poc.spec.ts  (projects: Chromium + WebKit; Chromium at an Android-ish viewport)
import { test, expect } from '@playwright/test';
import { startFakeHost } from './fixtures/fakeHost'; // local mock server serving /join and fake WSS responses

test('mobile_layout_has_no_horizontal_overflow', async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto('/join');
  await expect(page.getByText(/join/i)).toBeVisible();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
  expect(overflow).toBe(false);
});

test('keyboard_focus_is_visible', async ({ page }) => {
  await page.goto('/join');
  await page.keyboard.press('Tab');
  const outline = await page.evaluate(() => getComputedStyle(document.activeElement as Element).outlineStyle);
  expect(outline).not.toBe('none');
});
```

- [ ] **Step 4: Run E2E to verify it fails**

Run: `npm run test:e2e -- --project=chromium --project=webkit`
Expected: FAIL (no server/app wired). The local `@playwright/test` version is pinned; this does not implicitly download the latest.

- [ ] **Step 5: Wire the E2E harness**

Create `playwright.config.ts` with the two projects (Chromium at an Android-ish viewport, WebKit not treated as an iPhone), defining an explicit local `webServer` (the Vite musician dev server) and `baseURL`, plus a `startFakeHost` fixture that models the mocked `/join` + WSS responses; the tests wait on visible elements. Add `e2e/**` to Vitest's exclude so unit and E2E suites stay separate.

- [ ] **Step 6: Run the full automated gates**

Run: `npm run build:web`
Expected: PASS.
Run: `cargo build --workspace`
Expected: PASS (assets now exist).
Run: `cargo fmt --all -- --check`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.
Run: `cargo test --workspace`
Expected: PASS.
Run: `npm run typecheck`
Expected: PASS.
Run: `npm run test`
Expected: PASS.
Run: `npm run test:e2e -- --project=chromium --project=webkit`
Expected: PASS. Tag the E2E as mock-based and **not** fulfillment of the real wireless requirement; emulated WebKit is not an iPhone.

### Task 10: Real one-phone audio, then two-mix hardware qualification and handoff (manual; may be blocked)

**Files:**
- Modify: `docs/qualification/macos-poc.md`

**Interfaces:**
- Consumes: Tasks 1–9.
- Produces: a documented manual qualification record. **No automated assertion substitutes for the real hardware result.** If hardware is unavailable, Task 10 is recorded as **blocked, never a fabricated pass**, and all software tasks may still be complete.

- [ ] **Step 1: Operator prerequisites (explicit actions)**

Confirm: actual Soundcraft 22 MTK over USB; iPhone Safari and Android Chrome devices with wired earphones; a Wi-Fi or Ethernet AP on the selected LAN; an operator-configured certificate for the trusted IP/hostname. Install **iOS full root trust** and **Android Chrome trust** on the real devices as separate manual operator actions (requested in Task 0). If the selected interface changes, **reject a mismatched certificate and require operator reconfiguration — no automatic CA trust**.

- [ ] **Step 2: One-phone real USB audio path**

Run the app; operator selects the actual device, verifies USB mapping, labels sources, joins one phone, and listens. Expected: real captured audio leaves the phone output; page visible, screen on. Manually verify L/R mapping on both phones; driver labels are not proof and no sound interpretation is assumed.

- [ ] **Step 3: Two independent mixes and isolation**

Run two phones with different personal gains/mutes. Expected: each hears its own mix; changes are independent; both may listen to the **same shared source** with independent settings; silence on disconnect; explicit re-arm after interruption.

- [ ] **Step 4: Safe failure, output route, and re-arm**

Exercise Stop, disconnect/reconnect, device loss, and output-route change. Expected: Stop invokes the immediate JS local mute; that is **not** an instant analog-silence promise. On a **detectable** output-route change, require re-arm; if the change is not detected, record it as unsupported (best-effort detection, as the spec says). No backlog replay, no auto-unmute/auto-arm.

- [ ] **Step 5: Physical latency measurement (procedure only)**

Use a **matched ADC on the same capture clock**: place a mono test impulse on channel 1 as the analog reference and the **single chosen headphone channel L** on channel 2, record them simultaneously, and repeat on R independently and separately. Optionally use a third channel only if the ADC actually supports it; **do not assume a currently owned lab ADC**. Report median, p95, p99, worst excursion, and outage tracks; **agree the pass statistics before qualifying the ≤10 ms goal**.

- [ ] **Step 6: Handoff and blocked/complete status**

Record in `docs/qualification/macos-poc.md`: automated results, what remains hardware-only (real Soundcraft, real phones, real AP, real iOS/Android TLS trust, physical latency), and whether Task 10 **completed or is blocked for hardware**. State that no stage/qualification claim is made.

## Sources

Plan-level references (supplied and approved; **API integration dependencies are source-verified, NOT build-validated**):

- `README.md`, `docs/ARCHITECTURE.md`, `docs/DESIGN-SYSTEM.md` (in-repo).
- CPAL `DeviceTrait`: <https://docs.rs/cpal/0.18.2/cpal/traits/trait.DeviceTrait.html>
- rtrb 0.4.0: <https://docs.rs/rtrb/0.4.0/rtrb/>
- `opus` 0.4.0 source: <https://docs.rs/crate/opus/0.4.0/source/src/lib.rs>
- `opusic-sys` 0.7.3 build: <https://docs.rs/crate/opusic-sys/0.7.3/source/build.rs>
- str0m 0.24.1 writer: <https://docs.rs/crate/str0m/0.24.1/source/src/media/writer.rs>
- str0m codec_config: <https://docs.rs/crate/str0m/0.24.1/source/src/format/codec_config.rs>
- `is` 0.11.1 parse: <https://docs.rs/crate/is/0.11.1/source/src/parse.rs>
- `axum-server` `RustlsConfig`: <https://docs.rs/axum-server/0.8.0/axum_server/tls_rustls/struct.RustlsConfig.html>
- `rust-embed` 8.12.0: <https://docs.rs/crate/rust-embed/8.12.0/source/README.md>
- mkcert: <https://github.com/FiloSottile/mkcert>
- Apple certificate trust: <https://support.apple.com/en-us/102390>
- Apple `NSMicrophoneUsageDescription`: <https://developer.apple.com/documentation/bundleresources/information-property-list/nsmicrophoneusagedescription>
- Tauri capabilities/security: <https://v2.tauri.app/security/capabilities/>
- Tauri macOS application bundle (Info.plist merge, entitlements): <https://v2.tauri.app/distribute/macos-application-bundle/>
- Tauri config/CSP/requirements: <https://v2.tauri.app/>
- `tauri-build` `AppManifest`/`try_build` custom commands (verified as primary data; tauri-build 2.7.1 is separate from runtime Tauri 2.12.1): see Tauri v2 build documentation.
- `qrcode.react` 4.2.0 (peer React 19; operator-only pairing QR): npm registry, verified by parent `npm view`.
- Tailwind + Vite integration and npm workspaces: official docs (verified during Task 1).

These are supplied references reviewed as design inputs; API integration dependencies are **source-verified, not build-validated**. The `AppManifest::new().commands(&[...])` shape and the `tauri-build` 2.7.1 build dependency are verified primary data; the exact Tauri caller-window check API is verified during implementation, not guessed here.

## Execution Handoff

An execution method (parallel agents) was already supplied. **Plan complete and saved to `docs/superpowers/plans/2026-10-07-macos-poc-implementation.md`. Please review the plan against `docs/ARCHITECTURE.md` §1.1/§17.1 and `docs/DESIGN-SYSTEM.md`. Does it capture what you want?** No installation occurs until you approve. Framework/SDK versions above are a candidate baseline frozen in Task 1, not a compatibility claim, and every contract remains subject to review before implementation.

## Queued publication follow-up

After integrated implementation and verification, publish the intended changes to `main` in `https://github.com/daryllmagsombol/iem-cast.git`, with no force-push and no generated certificates, keys, credentials, or `.serena/` metadata staged. Recheck remote history before publishing; the repository was public and empty when inspected, and the active GitHub account had admin permission.

The user also selected **project documentation plus an interactive UI demo** for GitHub Pages. Queue that as a separate static-site deliverable after the POC: use the same design tokens and clearly label simulated controls, make no live USB/audio/LAN requests, link source documentation and releases without promising installers that do not exist, and link the deployed site from the repository homepage and README. The expected project URL is `https://daryllmagsombol.github.io/iem-cast/`; it is not deployed yet. The site's written scope and deployment plan require review before site implementation; this follow-up does not broaden the local IEM server's permissions or claim GitHub Pages can run Rust.
