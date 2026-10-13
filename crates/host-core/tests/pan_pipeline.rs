//! Task 8 integration proof: accepted linked-pair PAN/gain/mute settings traverse the REAL pipeline.
//!
//! This is software evidence only. It drives a real [`ControlActor`] through acceptance, installs
//! the actor's **accepted canonical snapshot** into a real [`Pipeline`] (engine + Opus encoder)
//! via a test-local [`DspControl`] adapter, injects deterministic 48 kHz capture blocks, and
//! measures the produced stereo. It does not prove hardware, device, network, or end-to-end
//! behavior, and makes no RTT/buffer latency claim.
//!
//! Observability at this seam:
//! - Pre-encode stereo PCM is publicly observable through [`MonitorPort`]; the pipeline taps it
//!   before encoding. Placement/mute/balance assertions are made on that PCM.
//! - Encoded [`EncodedFrame`]s (frame counts, start-sample timestamps, packet bytes) are public
//!   pipeline output. A real Opus decode cross-checks finiteness/side dominance with codec
//!   tolerance; exact codec PCM float equality is deliberately **not** asserted.
//!
//! Red-capability: the adapter reads `PAN_PIPELINE_MUTATION` (`omit_pan` | `reset_identity`) so a
//! controlled, test-local mutation can be toggled without editing production code. Default (unset)
//! is the real behavior; the mutation run is reported in the Task 8 report.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use host_core::audio::engine::MAX_SESSIONS;
use host_core::audio::{gain_linear, BUS_HEADROOM_DB, CODEC_FRAME_FRAMES};
use host_core::contract::{
    AudioEvent, CaptureBlock, CatalogSnapshot, Clock, ControlError, DspControl, EncodedFrame,
    ListenArm, MixPatch, MixSnapshot, SessionContext, SourceGain, SourceGainMatrix, SourceInfo,
    SourceRole, StereoSide, StereoFrame,
};
use host_core::control::ControlActor;
use host_core::ids::{
    ArmNonce, AudioEpoch, CatalogRevision, ChannelMapRevision, HostEpoch, MixRevision,
    SafetyGeneration, SessionEpoch, SourceId,
};
use host_core::monitor::MonitorPort;
use host_core::pipeline::{ListenerOutput, Pipeline};
use host_core::server::protocol::ServerMessage;

// ---------------------------------------------------------------------------------------------
// Fixed identities / fixtures
// ---------------------------------------------------------------------------------------------

/// Left pair member: intentionally the HIGHER physical index (3) vs the right member (1), and
/// non-adjacent, to prove the pair is carried by explicit side metadata, not index parity.
fn pair_left() -> SourceId {
    SourceId::from_bytes([0x41; 16])
}
fn pair_right() -> SourceId {
    SourceId::from_bytes([0x42; 16])
}
fn sess() -> SessionEpoch {
    SessionEpoch::from_bytes([0x22; 16])
}
fn audio_epoch() -> AudioEpoch {
    AudioEpoch::from_bytes([0x55; 16])
}

/// Capture channel count: the pair lives on physical indices 1 and 3, so four interleaved channels
/// are needed to address the higher-index left member.
const CAPTURE_CHANNELS: u16 = 4;
/// Left-content amplitude / right-content amplitude: distinct and low so the source sum never
/// reaches the limiter ceiling (worst-case peak < 0.11 << 0.501).
const LEFT_AMP: f32 = 0.2;
const RIGHT_AMP: f32 = 0.1;
const LEFT_GAIN_DB: f32 = -6.0;
const RIGHT_GAIN_DB: f32 = -12.0;
/// Deterministic probe tone; both members share the phase so each side is a clean sine.
const PROBE_HZ: f32 = 997.0;

#[derive(Debug)]
struct TestClock(Mutex<Instant>);
impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Instant::now())))
    }
}
impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.0.lock().expect("clock")
    }
}

fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog_revision: CatalogRevision(1),
        sources: vec![
            SourceInfo {
                source_id: pair_left(),
                physical_index: 3,
                label: "Pair L".to_string(),
                role: SourceRole::InputChannel,
                authorized: true,
                available: true,
                stereo_pair: Some(pair_right()),
                stereo_side: Some(StereoSide::Left),
                default_pan: 0.0,
            },
            SourceInfo {
                source_id: pair_right(),
                physical_index: 1,
                label: "Pair R".to_string(),
                role: SourceRole::InputChannel,
                authorized: true,
                available: true,
                stereo_pair: Some(pair_left()),
                stereo_side: Some(StereoSide::Right),
                default_pan: 0.0,
            },
        ],
    }
}

fn channel_map() -> Vec<host_core::contract::ChannelMapEntry> {
    use host_core::contract::ChannelMapEntry;
    vec![
        ChannelMapEntry {
            physical_index: 3,
            source_id: pair_left(),
            stereo_pair: Some(pair_right()),
            stereo_side: Some(StereoSide::Left),
            role: SourceRole::InputChannel,
        },
        ChannelMapEntry {
            physical_index: 1,
            source_id: pair_right(),
            stereo_pair: Some(pair_left()),
            stereo_side: Some(StereoSide::Right),
            role: SourceRole::InputChannel,
        },
    ]
}

fn gain(id: SourceId, gain_db: f32, muted: bool, pan: f32) -> SourceGain {
    SourceGain {
        source_id: id,
        gain_db,
        muted,
        pan,
    }
}

fn patch(
    base: MixRevision,
    left: SourceGain,
    right: SourceGain,
    master_db: f32,
    master_muted: bool,
) -> MixPatch {
    MixPatch {
        base_revision: base,
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[left, right]),
        master_db,
        master_muted,
    }
}

/// A four-channel block with the left member's content on physical 3 and the right member's on
/// physical 1; unmapped physical indices stay silent.
fn capture_block(start: u64, frames: u32, left_amp: f32, right_amp: f32) -> CaptureBlock {
    let mut samples = host_core::contract::silent_audio_slot();
    for frame in 0..frames as usize {
        let t = (start as usize + frame) as f32 / 48_000.0;
        let v = (2.0 * std::f32::consts::PI * PROBE_HZ * t).sin();
        samples[frame * CAPTURE_CHANNELS as usize + 3] = left_amp * v;
        samples[frame * CAPTURE_CHANNELS as usize + 1] = right_amp * v;
    }
    CaptureBlock {
        audio_epoch: audio_epoch(),
        start_sample: start,
        frame_count: frames,
        sample_rate_hz: 48_000,
        channel_count: CAPTURE_CHANNELS,
        channel_map_revision: ChannelMapRevision(1),
        samples,
    }
}

// ---------------------------------------------------------------------------------------------
// Controlled test-local mutation (default off)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    /// Adapter silently drops PAN (installs everything centered).
    OmitPan,
    /// Adapter forces a non-preserving unregister+register on every install, resetting partial and
    /// the per-slot encoder.
    ResetIdentity,
}

fn mutation() -> Mutation {
    match std::env::var("PAN_PIPELINE_MUTATION").ok().as_deref() {
        Some("omit_pan") => Mutation::OmitPan,
        Some("reset_identity") => Mutation::ResetIdentity,
        _ => Mutation::None,
    }
}

// ---------------------------------------------------------------------------------------------
// Test-local DspControl adapter that installs into a REAL Pipeline
// ---------------------------------------------------------------------------------------------

struct PipelineDsp {
    pipeline: Mutex<Pipeline>,
    snapshots: Mutex<[Option<MixSnapshot>; MAX_SESSIONS]>,
    installed: Mutex<Vec<MixSnapshot>>,
    arms: Mutex<Vec<ListenArm>>,
    slot: usize,
    mutation: Mutation,
}

impl PipelineDsp {
    fn new(channel_map: &[host_core::contract::ChannelMapEntry]) -> Self {
        Self {
            pipeline: Mutex::new(Pipeline::new(48_000, channel_map)),
            snapshots: Mutex::new([None, None, None, None]),
            installed: Mutex::new(Vec::new()),
            arms: Mutex::new(Vec::new()),
            slot: 0,
            mutation: mutation(),
        }
    }

    /// Install a snapshot into the real pipeline, mirroring the production `DspBridge` shape: the
    /// pipeline upsert plus the snapshot the adapter will hand `process`.
    fn install(&self, mut snapshot: MixSnapshot) -> Result<(), ControlError> {
        let session = snapshot.context.session_epoch;
        if self.mutation == Mutation::OmitPan {
            for entry in snapshot.sources.entries.iter_mut().take(snapshot.sources.len as usize) {
                entry.pan = 0.0;
            }
        }
        self.installed.lock().expect("installed").push(snapshot);

        let mut pipeline = self.pipeline.lock().expect("pipeline");
        if self.mutation == Mutation::ResetIdentity {
            pipeline.unregister(self.slot);
        }
        pipeline
            .register(self.slot, session, snapshot)
            .map_err(|_| ControlError::Internal)?;
        self.snapshots.lock().expect("snapshots")[self.slot] = Some(snapshot);
        Ok(())
    }

    fn install_count(&self) -> usize {
        self.installed.lock().expect("installed").len()
    }

    fn snapshots(&self) -> [Option<MixSnapshot>; MAX_SESSIONS] {
        *self.snapshots.lock().expect("snapshots")
    }

    fn with_pipeline<R>(&self, f: impl FnOnce(&mut Pipeline) -> R) -> R {
        let mut pipeline = self.pipeline.lock().expect("pipeline");
        f(&mut pipeline)
    }
}

impl DspControl for PipelineDsp {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError> {
        self.install(snapshot)
    }

    fn request_arm(&self, arm: ListenArm) -> Result<(), ControlError> {
        self.arms.lock().expect("arms").push(arm);
        // Mirror `DspBridge::request_arm`: re-install the live mix stamped with the NEW generation,
        // which changes the safety identity and therefore deliberately resets the engine stream.
        let current = self.snapshots()[self.slot];
        if let Some(mut snapshot) = current {
            snapshot.context = SessionContext {
                session_epoch: arm.context.session_epoch,
                audio_epoch: arm.context.audio_epoch,
                safety_generation: arm.context.safety_generation,
            };
            self.install(snapshot)?;
        }
        Ok(())
    }

    fn disarm(&self, _session: SessionEpoch, _new_generation: SafetyGeneration) {}
}

fn actor(dsp: &Arc<PipelineDsp>) -> ControlActor {
    let clock: Arc<dyn Clock> = TestClock::new();
    let dsp_trait: Arc<dyn DspControl> = dsp.clone();
    ControlActor::new(
        clock,
        dsp_trait,
        catalog(),
        HostEpoch::from_bytes([0x11; 16]),
        audio_epoch(),
    )
}

fn ctx(session: SessionEpoch, generation: u64) -> SessionContext {
    SessionContext {
        session_epoch: session,
        audio_epoch: audio_epoch(),
        safety_generation: SafetyGeneration(generation),
    }
}

// ---------------------------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct PcmTap {
    pcm: Mutex<Vec<f32>>,
}

impl MonitorPort for PcmTap {
    fn on_frames(&self, frames: &[StereoFrame]) {
        let mut pcm = self.pcm.lock().expect("pcm");
        for frame in frames {
            let n = frame.frame_count as usize * 2;
            pcm.extend_from_slice(&frame.pcm[..n]);
        }
    }
}

fn side_peaks(pcm: &[f32]) -> (f32, f32) {
    let mut lp = 0.0f32;
    let mut rp = 0.0f32;
    for frame in pcm.chunks_exact(2) {
        lp = lp.max(frame[0].abs());
        rp = rp.max(frame[1].abs());
    }
    (lp, rp)
}

struct Phase {
    pcm: Vec<f32>,
    cadence: Vec<(u64, u32)>,
    encoded: Vec<EncodedFrame>,
}

/// Drive `warmup + blocks` capture blocks of `frames` each. Warmup uses no monitor (settles ramps);
/// the measured blocks are tapped and their encoded frames collected.
fn measure(
    dsp: &PipelineDsp,
    start: &mut u64,
    frames: u32,
    left_amp: f32,
    right_amp: f32,
    warmup_blocks: usize,
    blocks: usize,
) -> Phase {
    for _ in 0..warmup_blocks {
        let block = capture_block(*start, frames, left_amp, right_amp);
        *start += frames as u64;
        let snapshots = dsp.snapshots();
        dsp.with_pipeline(|pipe| {
            let mut out: [ListenerOutput; MAX_SESSIONS] = std::array::from_fn(ListenerOutput::empty);
            pipe.process(&block, &snapshots, &mut out, None).unwrap();
        });
    }

    let tap = Arc::new(PcmTap::default());
    let port: &dyn MonitorPort = tap.as_ref();
    let mut cadence = Vec::new();
    let mut encoded = Vec::new();
    for _ in 0..blocks {
        let block = capture_block(*start, frames, left_amp, right_amp);
        *start += frames as u64;
        let snapshots = dsp.snapshots();
        dsp.with_pipeline(|pipe| {
            let mut out: [ListenerOutput; MAX_SESSIONS] = std::array::from_fn(ListenerOutput::empty);
            pipe.process(&block, &snapshots, &mut out, Some((0, port))).unwrap();
            for frame in out[0].frames.iter().take(out[0].count) {
                assert!(frame.packet.len > 0, "every produced frame must encode");
                cadence.push((frame.start_sample, frame.frame_count));
                encoded.push(*frame);
            }
        });
    }
    let pcm = tap.pcm.lock().expect("pcm").clone();
    Phase {
        pcm,
        cadence,
        encoded,
    }
}

/// Decode the encoded frames with the real Opus decoder and return the per-side peaks. Codec
/// tolerances apply; this only cross-checks finiteness and gross side dominance.
fn decode_peaks(frames: &[EncodedFrame]) -> (f32, f32) {
    use opus::{Channels, Decoder};
    let mut decoder = Decoder::new(48_000, Channels::Stereo).expect("opus decoder");
    let mut buf = vec![0f32; CODEC_FRAME_FRAMES as usize * 2];
    let mut lp = 0.0f32;
    let mut rp = 0.0f32;
    for frame in frames {
        let n = decoder
            .decode_float(&frame.packet.bytes[..frame.packet.len as usize], &mut buf, false)
            .expect("decode");
        for i in 0..n {
            lp = lp.max(buf[i * 2].abs());
            rp = rp.max(buf[i * 2 + 1].abs());
        }
    }
    (lp, rp)
}

fn assert_close(measured: f32, expected: f32, rel: f32, what: &str) {
    let err = (measured - expected).abs();
    assert!(
        err <= expected.abs() * rel + 1e-6,
        "{what}: measured {measured:.6} differs from expected {expected:.6} by {err:.6} (rel limit {rel:.4})"
    );
}

/// Assert the emitted codec frames form a contiguous, gapless timeline: every frame is a full
/// [`CODEC_FRAME_FRAMES`]-frame packet and consecutive frames are spaced exactly by it.
fn assert_contiguous(cadence: &[(u64, u32)], what: &str) {
    assert!(!cadence.is_empty(), "{what}: expected at least one frame");
    for (_, count) in cadence {
        assert_eq!(*count, CODEC_FRAME_FRAMES, "{what}: frame size");
    }
    for pair in cadence.windows(2) {
        assert_eq!(
            pair[1].0 - pair[0].0,
            CODEC_FRAME_FRAMES as u64,
            "{what}: non-contiguous timestamps {:?} -> {:?}",
            pair[0],
            pair[1]
        );
    }
}

// Expected steady-state peaks from the mix law: member contribution =
// amp * gain * pan_coef * bus headroom.
fn expected_side(left_amp: f32, right_amp: f32, pan: f32) -> (f32, f32) {
    // PairLeft routes only to L via L(p) = 1 - max(p,0); PairRight only to R via R(p) = 1 + min(p,0).
    let l_coef = 1.0 - pan.max(0.0);
    let r_coef = 1.0 + pan.min(0.0);
    let headroom = gain_linear(BUS_HEADROOM_DB);
    let left = left_amp * gain_linear(LEFT_GAIN_DB) * l_coef * headroom;
    let right = right_amp * gain_linear(RIGHT_GAIN_DB) * r_coef * headroom;
    (left, right)
}

// ---------------------------------------------------------------------------------------------
// Test 1: acceptance -> canonical snapshot -> real pipeline -> placement / mute / balance
// ---------------------------------------------------------------------------------------------

#[test]
fn linked_pair_pan_gain_mute_traverses_real_pipeline() {
    let dsp = Arc::new(PipelineDsp::new(&channel_map()));
    let mut actor = actor(&dsp);

    // Initial muted mix: equal canonical PAN, independent gains/mutes, master muted. Muting while
    // unarmed is always allowed.
    let a1 = actor
        .apply_patch(
            sess(),
            patch(
                MixRevision(0),
                gain(pair_left(), LEFT_GAIN_DB, true, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, true, 0.0),
                -6.0,
                true,
            ),
        )
        .expect("muted initial mix is accepted");

    // The accepted canonical snapshot carries the pair with equal PAN and independent members.
    let snap = a1.canonical_snapshot;
    let entries = snap.sources.as_slice();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].pan, entries[1].pan, "linked pair shares one canonical PAN");
    assert_eq!(entries[0].gain_db, LEFT_GAIN_DB);
    assert_eq!(entries[1].gain_db, RIGHT_GAIN_DB);
    assert_eq!(snap.context, ctx(sess(), 0));

    // A PAN-only edit while unarmed is allowed and must NOT arm the session or unlock unmute: the
    // sticky permission gate is intact.
    let a2 = actor
        .apply_patch(
            sess(),
            patch(
                a1.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, true, -0.5),
                gain(pair_right(), RIGHT_GAIN_DB, true, -0.5),
                -6.0,
                true,
            ),
        )
        .expect("PAN edit is allowed while unarmed");
    assert_eq!(a2.accepted_revision, MixRevision(2));
    assert_eq!(
        actor.current_generation(sess()),
        SafetyGeneration(0),
        "a PAN edit must not assign a safety generation / arm the session"
    );
    let forbidden = actor.apply_patch(
        sess(),
        patch(
            a2.accepted_revision,
            gain(pair_left(), LEFT_GAIN_DB, false, -0.5),
            gain(pair_right(), RIGHT_GAIN_DB, true, -0.5),
            -6.0,
            true,
        ),
    );
    assert_eq!(
        forbidden,
        Err(ControlError::UnarmedUnmuteForbidden),
        "a PAN edit must not grant unmute"
    );

    // Arm through the existing permission gate.
    let nonce = ArmNonce::from_bytes([0x66; 16]);
    actor
        .arm(
            sess(),
            ListenArm {
                context: ctx(sess(), 0),
                applied_revision: a2.accepted_revision,
                arm_nonce: nonce,
            },
        )
        .expect("arm at generation 0");
    assert_eq!(actor.current_generation(sess()), SafetyGeneration(1));
    assert!(
        matches!(
            actor.on_audio_event(AudioEvent::ArmApplied {
                context: ctx(sess(), 1),
                arm_nonce: nonce,
            }),
            Some(ServerMessage::ListenArmed(_))
        ),
        "the exact pending arm tuple must confirm"
    );

    // With the gate open, unmute both members and raise master; PAN recentered. Stamped generation 1.
    let a4 = actor
        .apply_patch(
            sess(),
            patch(
                MixRevision(2),
                gain(pair_left(), LEFT_GAIN_DB, false, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, false, 0.0),
                0.0,
                false,
            ),
        )
        .expect("unmute while armed is accepted");
    assert_eq!(a4.canonical_snapshot.context, ctx(sess(), 1));
    assert!(dsp.install_count() >= 4, "each accepted patch installs into the pipeline");

    let mut start = 0u64;

    // --- Placement: center (PAN 0) => left content on L, right content on R. ---
    let center = measure(&dsp, &mut start, 128, LEFT_AMP, RIGHT_AMP, 6, 4);
    assert!(
        center.pcm.iter().all(|s| s.is_finite()),
        "pre-encode PCM must be finite"
    );
    let (cl, cr) = side_peaks(&center.pcm);
    let (el, er) = expected_side(LEFT_AMP, RIGHT_AMP, 0.0);
    // Tolerance 5%: pre-encode mix math is deterministic, but sampling a 997 Hz sine does not
    // always land on its crest, plus f32 rounding and ramp/limiter settle. No codec is involved.
    assert_close(cl, el, 0.05, "center L peak");
    assert_close(cr, er, 0.05, "center R peak");
    assert!(cl > cr, "distinct member gains must be preserved (left louder)");
    assert_contiguous(&center.cadence, "center phase");

    // --- Extreme left (PAN -1): right member fully suppressed, left content intact. ---
    let a5 = actor
        .apply_patch(
            sess(),
            patch(
                a4.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, false, -1.0),
                gain(pair_right(), RIGHT_GAIN_DB, false, -1.0),
                0.0,
                false,
            ),
        )
        .expect("extreme-left balance edit");
    let left = measure(&dsp, &mut start, 128, LEFT_AMP, RIGHT_AMP, 6, 4);
    let (ll, lr) = side_peaks(&left.pcm);
    let (ell, _elr) = expected_side(LEFT_AMP, RIGHT_AMP, -1.0);
    assert_close(ll, ell, 0.05, "extreme-left L peak");
    assert!(
        lr < 1e-4,
        "at full-left balance the right member must be silent, got {lr}"
    );
    let (dl, dr) = decode_peaks(&left.encoded);
    assert!(dl.is_finite() && dr.is_finite(), "decoded output must be finite");
    assert!(
        dl > dr * 2.0,
        "the real codec path must preserve left dominance under full-left balance: L={dl} R={dr}"
    );
    // Encoded frame timestamps stay contiguous across the balance step.
    assert_contiguous(&left.cadence, "extreme-left phase");

    // --- Extreme right (PAN +1): left member fully suppressed. ---
    let a6 = actor
        .apply_patch(
            sess(),
            patch(
                a5.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, false, 1.0),
                gain(pair_right(), RIGHT_GAIN_DB, false, 1.0),
                0.0,
                false,
            ),
        )
        .expect("extreme-right balance edit");
    let right = measure(&dsp, &mut start, 128, LEFT_AMP, RIGHT_AMP, 6, 4);
    let (rl, rr) = side_peaks(&right.pcm);
    let (_erl, err_expected) = expected_side(LEFT_AMP, RIGHT_AMP, 1.0);
    assert!(
        rl < 1e-4,
        "at full-right balance the left member must be silent, got {rl}"
    );
    assert_close(rr, err_expected, 0.05, "extreme-right R peak");

    // --- No crossfeed: with PAN centered, a signal on only one member's physical channel must not
    //     appear on the opposite output side. ---
    let a7 = actor
        .apply_patch(
            sess(),
            patch(
                a6.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, false, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, false, 0.0),
                0.0,
                false,
            ),
        )
        .expect("recenter balance");
    let left_only = measure(&dsp, &mut start, 128, LEFT_AMP, 0.0, 6, 4);
    let (xll, xlr) = side_peaks(&left_only.pcm);
    assert!(xll > 0.02, "left member drives L; got {xll}");
    assert!(
        xlr < 1e-4,
        "left member must not crossfeed into R, got {xlr}"
    );
    let right_only = measure(&dsp, &mut start, 128, 0.0, RIGHT_AMP, 6, 4);
    let (xrl, xrr) = side_peaks(&right_only.pcm);
    assert!(
        xrl < 1e-4,
        "right member must not crossfeed into L, got {xrl}"
    );
    assert!(xrr > 0.008, "right member drives R; got {xrr}");

    // --- Independent mute on a linked pair: muting one member leaves the other member's signal. ---
    let a8 = actor
        .apply_patch(
            sess(),
            patch(
                a7.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, false, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, true, 0.0),
                0.0,
                false,
            ),
        )
        .expect("mute the right member only");
    let muted_right = measure(&dsp, &mut start, 128, LEFT_AMP, RIGHT_AMP, 8, 4);
    let (mrl, mrr) = side_peaks(&muted_right.pcm);
    assert!(
        mrl > 0.02,
        "muting the right member must leave the left member's signal, got L={mrl}"
    );
    assert!(
        mrr < 1e-3,
        "muting the right member must silence only it, got R={mrr}"
    );

    // Inverse: mute left, unmute right.
    let a9 = actor
        .apply_patch(
            sess(),
            patch(
                a8.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, true, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, false, 0.0),
                0.0,
                false,
            ),
        )
        .expect("mute the left member only");
    assert_eq!(a9.canonical_snapshot.context, ctx(sess(), 1));
    let muted_left = measure(&dsp, &mut start, 128, LEFT_AMP, RIGHT_AMP, 8, 4);
    let (ml_l, ml_r) = side_peaks(&muted_left.pcm);
    assert!(
        ml_l < 1e-3,
        "muting the left member must silence only it, got L={ml_l}"
    );
    assert!(
        ml_r > 0.008,
        "the right member's signal survives, got R={ml_r}"
    );

    // Dynamic balance was maintained with one canonical PAN across the whole run: every accepted
    // snapshot kept the pair equal-PAN with independent members.
    let final_snap = actor.accepted_snapshot(sess()).expect("accepted snapshot");
    let final_entries = final_snap.sources.as_slice();
    assert_eq!(final_entries[0].pan, final_entries[1].pan);
    assert_ne!(final_entries[0].gain_db, final_entries[1].gain_db);
    assert_ne!(final_entries[0].muted, final_entries[1].muted);
}

// ---------------------------------------------------------------------------------------------
// Test 2: a same-context PAN edit preserves partial PCM / encoder continuity (not retag/replay)
// ---------------------------------------------------------------------------------------------

#[test]
fn same_context_pan_edit_preserves_pipeline_continuity() {
    let dsp = Arc::new(PipelineDsp::new(&channel_map()));
    let mut actor = actor(&dsp);

    let a1 = actor
        .apply_patch(
            sess(),
            patch(
                MixRevision(0),
                gain(pair_left(), LEFT_GAIN_DB, true, 0.0),
                gain(pair_right(), RIGHT_GAIN_DB, true, 0.0),
                -6.0,
                true,
            ),
        )
        .expect("muted initial mix");
    let nonce = ArmNonce::from_bytes([0x66; 16]);
    actor
        .arm(
            sess(),
            ListenArm {
                context: ctx(sess(), 0),
                applied_revision: a1.accepted_revision,
                arm_nonce: nonce,
            },
        )
        .expect("arm");
    actor.on_audio_event(AudioEvent::ArmApplied {
        context: ctx(sess(), 1),
        arm_nonce: nonce,
    });

    // Hold a 100-frame partial (less than one 120-frame codec frame), so nothing is emitted yet.
    let mut out: [ListenerOutput; MAX_SESSIONS] = std::array::from_fn(ListenerOutput::empty);
    let snapshots = dsp.snapshots();
    let block_a = capture_block(0, 100, LEFT_AMP, RIGHT_AMP);
    dsp.with_pipeline(|pipe| {
        pipe.process(&block_a, &snapshots, &mut out, None).unwrap();
    });
    assert_eq!(out[0].count, 0, "a 100-frame block cannot complete a codec frame");

    // PAN-only edit with the SAME safety context through the real control actor.
    let a2 = actor
        .apply_patch(
            sess(),
            patch(
                a1.accepted_revision,
                gain(pair_left(), LEFT_GAIN_DB, true, -0.7),
                gain(pair_right(), RIGHT_GAIN_DB, true, -0.7),
                -6.0,
                true,
            ),
        )
        .expect("same-context PAN edit");
    assert_eq!(a2.accepted_revision, MixRevision(2));
    assert_eq!(
        a2.canonical_snapshot.context,
        ctx(sess(), 1),
        "an ordinary PAN edit keeps the exact safety context"
    );

    // 20 more contiguous frames complete the held 100-frame partial: the emitted frame is anchored
    // to its pre-edit start (0), proving the partial and per-slot encoder survived the edit rather
    // than being reset, retagged, or replayed.
    let snapshots = dsp.snapshots();
    let block_b = capture_block(100, 20, LEFT_AMP, RIGHT_AMP);
    dsp.with_pipeline(|pipe| {
        pipe.process(&block_b, &snapshots, &mut out, None).unwrap();
    });
    assert_eq!(
        out[0].count, 1,
        "the held partial must complete across the PAN edit"
    );
    let frame = out[0].frames[0];
    assert_eq!(
        frame.start_sample, 0,
        "the frame keeps its original timeline anchor, not a retagged/replayed start"
    );
    assert_eq!(frame.frame_count, CODEC_FRAME_FRAMES);
    assert!(frame.packet.len > 0, "the encoder produced a real packet");
    assert_eq!(frame.context, ctx(sess(), 1));

    // The timebase stays contiguous afterward: the next full frame is anchored at 120.
    let snapshots = dsp.snapshots();
    let block_c = capture_block(120, 120, LEFT_AMP, RIGHT_AMP);
    dsp.with_pipeline(|pipe| {
        pipe.process(&block_c, &snapshots, &mut out, None).unwrap();
    });
    assert_eq!(out[0].count, 1);
    assert_eq!(out[0].frames[0].start_sample, 120);
}
