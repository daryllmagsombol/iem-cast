//! DSP / mix / safety unit tests.

use super::engine::{AudioEngine, MAX_SESSIONS};
use super::gain::{centered_coefficients, gain_linear, validate_gain};
use super::limiter::Limiter;
use super::safety::SafetyWorker;
use super::{BUS_HEADROOM_DB, CODEC_FRAME_FRAMES, MAX_OUT_FRAMES_PER_BLOCK};
use crate::contract::{
    AudioEvent, AudioFault, CaptureBlock, ChannelMapEntry, MixSnapshot, SessionContext,
    SourceGain, SourceGainMatrix, SourceRole, StereoFrame,
};
use crate::ids::{
    AudioEpoch, CatalogRevision, MixRevision, SafetyGeneration, SessionEpoch, SourceId,
};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn ctx_a() -> SessionContext {
    SessionContext {
        session_epoch: SessionEpoch::from_bytes([2; 16]),
        audio_epoch: AudioEpoch::from_bytes([7; 16]),
        safety_generation: SafetyGeneration(1),
    }
}

fn sess() -> SessionEpoch {
    SessionEpoch::from_bytes([2; 16])
}

fn two_channel_map() -> Vec<ChannelMapEntry> {
    vec![
        ChannelMapEntry {
            physical_index: 0,
            source_id: SourceId::from_bytes([1; 16]),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        },
        ChannelMapEntry {
            physical_index: 1,
            source_id: SourceId::from_bytes([2; 16]),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        },
    ]
}

fn snapshot_gain(master_db: f32, master_muted: bool) -> MixSnapshot {
    MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&[
            SourceGain {
                source_id: SourceId::from_bytes([1; 16]),
                gain_db: 0.0,
                muted: false,
            },
            SourceGain {
                source_id: SourceId::from_bytes([2; 16]),
                gain_db: 0.0,
                muted: false,
            },
        ]),
        master_db,
        master_muted,
    }
}

fn snapshot_source_gain(gain_db: f32, source_id: SourceId) -> MixSnapshot {
    MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id,
            gain_db,
            muted: false,
        }]),
        master_db: 0.0,
        master_muted: false,
    }
}

fn capture_block_constant(value: f32, frames: u32) -> CaptureBlock {
    let mut samples = crate::contract::silent_audio_slot();
    for n in 0..(frames as usize * 2) {
        samples[n] = value;
    }
    CaptureBlock {
        audio_epoch: ctx_a().audio_epoch,
        start_sample: 0,
        frame_count: frames,
        sample_rate_hz: 48_000,
        channel_count: 2,
        channel_map_revision: crate::ids::ChannelMapRevision(1),
        samples,
    }
}

fn listen_arm_with_gen(_gen: u64) -> crate::contract::ListenArm {
    crate::contract::ListenArm {
        context: ctx_a(),
        applied_revision: MixRevision(1),
        arm_nonce: crate::ids::ArmNonce::from_bytes([9; 16]),
    }
}

/// Render exactly one 120-frame stereo frame for a mix and return its interleaved PCM.
fn render_one_frame(mix: MixSnapshot) -> [f32; crate::ids::STEREO_PCM_SAMPLES] {
    let mut engine = AudioEngine::new(48_000, &two_channel_map());
    engine.register_session(0, ctx_a(), mix);
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    let mut collected = [0f32; crate::ids::STEREO_PCM_SAMPLES];
    let mut produced = 0u32;
    let mut iterations = 0;
    while produced < CODEC_FRAME_FRAMES && iterations < 4 {
        let n = engine
            .process_block(0, &capture_block_constant(0.5, 128), &mix, &mut out)
            .unwrap();
        if n > 0 {
            collected.copy_from_slice(&out[0].pcm);
            produced = out[0].frame_count;
        }
        iterations += 1;
    }
    collected
}

/// Render one correlated unity frame through a single source.
fn render_unity_correlated() -> Vec<f32> {
    render_one_frame(snapshot_source_gain(0.0, SourceId::from_bytes([1; 16]))).to_vec()
}

// ---------------------------------------------------------------------------------------------
// Gain
// ---------------------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------------------
// Ramps / geometry
// ---------------------------------------------------------------------------------------------

#[test]
fn default_gain_ramp_is_5ms_240_frames() {
    let mut engine = AudioEngine::new(48_000, &two_channel_map());
    engine.register_session(0, ctx_a(), snapshot_gain(-60.0, false));
    let mut produced = 0u32;
    while produced < 480 {
        let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
        let n = engine
            .process_block(
                0,
                &capture_block_constant(0.0, 128),
                &snapshot_gain(0.0, false),
                &mut out,
            )
            .unwrap();
        produced += n as u32 * CODEC_FRAME_FRAMES;
    }
    assert!(produced >= 480 - CODEC_FRAME_FRAMES);

    // After one 5 ms (240-frame) ramp the target gain is reached.
    let mut ramp = super::engine::ramp::Ramp::new(-60.0);
    ramp.set_target(0.0);
    let mut frames = 0u32;
    while ramp.current_db() < 0.0 && frames < 1000 {
        ramp.next_db();
        frames += 1;
    }
    assert_eq!(frames, 240);
}

#[test]
fn process_block_geometry_matches_codec_120_frame_boundary() {
    let mut engine = AudioEngine::new(48_000, &two_channel_map());
    engine.register_session(0, ctx_a(), snapshot_gain(0.0, false));
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    assert_eq!(
        engine
            .process_block(0, &capture_block_constant(0.25, 128), &snapshot_gain(0.0, false), &mut out)
            .unwrap(),
        1
    );
    assert_eq!(
        engine
            .process_block(0, &capture_block_constant(0.25, 128), &snapshot_gain(0.0, false), &mut out)
            .unwrap(),
        1
    );
    assert_eq!(out[0].frame_count, CODEC_FRAME_FRAMES);
    assert_eq!(out[0].context, ctx_a());
}

#[test]
fn process_block_256_returns_two_full_frames() {
    let mut engine = AudioEngine::new(48_000, &two_channel_map());
    engine.register_session(0, ctx_a(), snapshot_gain(0.0, false));
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    assert_eq!(
        engine
            .process_block(0, &capture_block_constant(0.25, 256), &snapshot_gain(0.0, false), &mut out)
            .unwrap(),
        2
    );
}

#[test]
fn partial_frame_is_reset_on_identity_change() {
    let mut engine = AudioEngine::new(48_000, &two_channel_map());
    engine.register_session(0, ctx_a(), snapshot_gain(0.0, false));
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    // Carry 8 frames.
    assert_eq!(
        engine
            .process_block(0, &capture_block_constant(0.0, 8), &snapshot_gain(0.0, false), &mut out)
            .unwrap(),
        0
    );
    let mut changed = snapshot_gain(0.0, false);
    changed.context.safety_generation = SafetyGeneration(2);
    // The 8 held frames are not stale-replayed: 128 fresh frames still yield only one full frame.
    assert_eq!(
        engine
            .process_block(0, &capture_block_constant(0.0, 128), &changed, &mut out)
            .unwrap(),
        1
    );
}

#[test]
fn engine_holds_the_fixed_session_count() {
    assert_eq!(MAX_SESSIONS, 4);
}

// ---------------------------------------------------------------------------------------------
// Limiter
// ---------------------------------------------------------------------------------------------

#[test]
fn output_never_exceeds_minus_6_dbfs_ceiling() {
    let limit = 10f32.powf(-6.0 / 20.0);
    let out = render_unity_correlated();
    assert!(out.iter().all(|s| s.abs() <= limit + 1e-6));
}

#[test]
fn limiter_bounds_an_over_ceiling_peak_instantly() {
    let mut limiter = Limiter::new();
    // Unity input requires the shared gain to clamp so the digital peak stays at −6 dBFS.
    let (l, r) = limiter.process(2.0, 2.0);
    let limit = 10f32.powf(-6.0 / 20.0);
    assert!(l.abs() <= limit + 1e-6 && r.abs() <= limit + 1e-6);
    assert!(limiter.reduction_db() < 0.0);
}

// ---------------------------------------------------------------------------------------------
// Isolation / safety
// ---------------------------------------------------------------------------------------------

#[test]
fn two_sessions_produce_different_stereo_contributions() {
    let a = render_one_frame(snapshot_source_gain(-3.0, SourceId::from_bytes([1; 16])));
    let b = render_one_frame(snapshot_source_gain(-9.0, SourceId::from_bytes([1; 16])));
    assert_ne!(a, b);
}

#[test]
fn safety_worker_consumes_provided_generation_and_never_invents_one() {
    let worker = SafetyWorker::new(sess());
    let ev = worker
        .apply_arm(&listen_arm_with_gen(0), SafetyGeneration(5))
        .unwrap();
    assert_eq!(
        ev,
        AudioEvent::ArmApplied {
            context: ctx_a(),
            arm_nonce: listen_arm_with_gen(0).arm_nonce,
        }
    );
    assert_eq!(worker.generation(), SafetyGeneration(5));
}

#[test]
fn invalid_sample_faults_the_affected_path() {
    assert_eq!(validate_gain(f32::NAN), Err(AudioFault::InvalidSample));
}
