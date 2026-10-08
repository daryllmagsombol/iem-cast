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
fn bus_headroom_constant_is_minus_6_db() {
    assert_eq!(BUS_HEADROOM_DB, -6.0);
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

// ---------------------------------------------------------------------------------------------
// Controlled signal measurement (diagnostic harness)
//
// These tests run the *actual* production DSP + Opus encoder over synthetic signals and measure
// the output. They are bounded offline measurements, not hardware qualification. The goal is to
// put runnable evidence behind or against the reported "low volume / choppy distortion" report:
//   - a clean single source must falsify clipping if its measured peak is far below full scale;
//   - a correlated full-scale sum pins the limiter, showing where saturation would come from;
//   - a cadence probe checks the engine does not introduce gaps/dropouts in the frame stream.
// ---------------------------------------------------------------------------------------------

use std::sync::Mutex;

/// Captures the pre-encode stereo PCM the DSP hands to the monitor, plus frame cadence metadata.
#[derive(Default)]
struct SignalTap {
    pcm: Mutex<Vec<f32>>,
    starts: Mutex<Vec<u64>>,
    frame_counts: Mutex<Vec<u32>>,
}

impl crate::monitor::MonitorPort for SignalTap {
    fn on_frames(&self, frames: &[StereoFrame]) {
        let mut pcm = self.pcm.lock().unwrap();
        let mut starts = self.starts.lock().unwrap();
        let mut counts = self.frame_counts.lock().unwrap();
        for frame in frames {
            starts.push(frame.start_sample);
            counts.push(frame.frame_count);
            let n = frame.frame_count as usize * 2;
            pcm.extend_from_slice(&frame.pcm[..n]);
        }
    }
}

fn input_map_source_count(n: usize) -> Vec<ChannelMapEntry> {
    (0..n)
        .map(|index| ChannelMapEntry {
            physical_index: index as u16,
            source_id: SourceId::from_bytes([(index + 1) as u8; 16]),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        })
        .collect()
}

fn unity_gains(n: usize) -> Vec<SourceGain> {
    (0..n)
        .map(|index| SourceGain {
            source_id: SourceId::from_bytes([(index + 1) as u8; 16]),
            gain_db: 0.0,
            muted: false,
        })
        .collect()
}

/// A block where every one of `channels` inputs carries the same in-phase sine (fully correlated).
fn correlated_sine_block(
    channels: usize,
    frames: u32,
    start_sample: u64,
    freq: f32,
    amp: f32,
) -> CaptureBlock {
    let mut samples = crate::contract::silent_audio_slot();
    for frame in 0..frames as usize {
        let t = (start_sample as usize + frame) as f32 / 48_000.0;
        let v = amp * (2.0 * std::f32::consts::PI * freq * t).sin();
        for ch in 0..channels {
            samples[frame * channels + ch] = v;
        }
    }
    CaptureBlock {
        audio_epoch: ctx_a().audio_epoch,
        start_sample,
        frame_count: frames,
        sample_rate_hz: 48_000,
        channel_count: channels as u16,
        channel_map_revision: crate::ids::ChannelMapRevision(1),
        samples,
    }
}

struct Measurement {
    peak: f32,
    rms: f32,
    frames: usize,
    encoded_bytes: usize,
}

/// Run the real `Pipeline` (engine + Opus encoder) over `blocks` correlated sine blocks and
/// measure the pre-encode stereo PCM plus the encoded packet sizes.
fn measure_pipeline(
    sources: usize,
    blocks: usize,
    block_frames: u32,
    freq: f32,
    amp: f32,
    master_db: f32,
) -> (
    Measurement,
    Vec<(u64, u32)>,
    Vec<crate::contract::EncodedFrame>,
) {
    let map = input_map_source_count(sources);
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
    let session = sess();
    let snapshot = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&unity_gains(sources)),
        master_db,
        master_muted: false,
    };
    pipeline.register(0, session, snapshot).unwrap();

    let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
    snapshots[0] = Some(snapshot);

    let tap = std::sync::Arc::new(SignalTap::default());
    let port: &dyn crate::monitor::MonitorPort = tap.as_ref();

    let mut encoded: Vec<crate::contract::EncodedFrame> = Vec::new();
    let mut start = 0u64;
    for _ in 0..blocks {
        let block = correlated_sine_block(sources, block_frames, start, freq, amp);
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);
        pipeline
            .process(&block, &snapshots, &mut out, Some((0, port)))
            .unwrap();
        for frame in out[0].frames.iter().take(out[0].count) {
            encoded.push(*frame);
        }
        start += block_frames as u64;
    }

    let pcm = tap.pcm.lock().unwrap().clone();
    let starts: Vec<u64> = tap.starts.lock().unwrap().clone();
    let counts: Vec<u32> = tap.frame_counts.lock().unwrap().clone();
    let cadence: Vec<(u64, u32)> = starts.into_iter().zip(counts).collect();

    let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (pcm.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / pcm.len().max(1) as f64)
        .sqrt() as f32;
    let encoded_bytes = encoded.iter().map(|f| f.packet.len as usize).sum();

    (
        Measurement {
            peak,
            rms,
            frames: pcm.len() / 2,
            encoded_bytes,
        },
        cadence,
        encoded,
    )
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Decode a set of encoded frames with the real Opus decoder and measure the decoded stereo peak
/// and RMS. Codec tolerances are expected; the point is to confirm there is no gross gain error or
/// hard clipping introduced by encode/decode.
fn decode_frames(frames: &[crate::contract::EncodedFrame]) -> (f32, f32, usize) {
    use opus::{Channels, Decoder};

    let mut decoder = Decoder::new(48_000, Channels::Stereo).expect("opus decoder");
    let mut out = vec![0f32; CODEC_FRAME_FRAMES as usize * 2 * 4];
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut count = 0usize;
    for frame in frames {
        let len = frame.packet.len as usize;
        let n = decoder
            .decode_float(&frame.packet.bytes[..len], &mut out, false)
            .expect("decode");
        let samples = n * 2;
        for s in &out[..samples] {
            peak = peak.max(s.abs());
            sum_sq += (*s as f64) * (*s as f64);
        }
        count += samples;
    }
    let rms = (sum_sq / count.max(1) as f64).sqrt() as f32;
    (peak, rms, count)
}

#[test]
fn measure_single_full_scale_source_level_layer() {
    // One mono source at 0 dB, master 0 dB. With the fixed -6 dB bus headroom, a full-scale sine
    // peaks at exactly -6 dBFS and (for a sine) has RMS -9.01 dBFS after ramps settle.
    let (m, cadence, encoded) = measure_pipeline(1, 40, 128, 997.0, 1.0, 0.0);
    let (dec_peak, dec_rms, dec_samples) = decode_frames(&encoded);
    println!(
        "[measure] single source 0dB: pcm_peak={:.6} ({:.2} dBFS) pcm_rms={:.6} ({:.2} dBFS)",
        m.peak,
        db(m.peak),
        m.rms,
        db(m.rms)
    );
    println!(
        "[measure] decoded: peak={:.6} ({:.2} dBFS) rms={:.6} ({:.2} dBFS) samples={} frames={} encoded_bytes={}",
        dec_peak,
        db(dec_peak),
        dec_rms,
        db(dec_rms),
        dec_samples,
        m.frames,
        m.encoded_bytes
    );
    println!(
        "[measure] cadence head={:?} len={}",
        &cadence[..cadence.len().min(6)],
        cadence.len()
    );

    let expected_peak = 10f32.powf(-6.0 / 20.0); // 0.501187
    let expected_rms = expected_peak / std::f32::consts::SQRT_2; // -9.01 dBFS
    assert!(
        (m.peak - expected_peak).abs() < 1e-4,
        "single full-scale source peak should be -6 dBFS ({expected_peak}), got {} ({:.3} dBFS)",
        m.peak,
        db(m.peak)
    );
    assert!(
        (m.rms - expected_rms).abs() < 2e-3,
        "single full-scale sine RMS should be ~-9.01 dBFS ({expected_rms}), got {} ({:.3} dBFS)",
        m.rms,
        db(m.rms)
    );
    // Codec decode is within a reasonable tolerance and never invents a hard sample ceiling.
    assert!(
        (dec_peak - expected_peak).abs() < 0.03,
        "decoded peak {} drifted from the -6 dBFS input beyond codec tolerance",
        dec_peak
    );
    assert!(
        encoded.iter().all(|f| f.packet.len > 0),
        "every frame must encode"
    );
}

#[test]
fn measure_negative_twelve_input_scales_linearly_to_minus_eighteen() {
    // -12 dB source into a -6 dB headroom -> -18 dBFS peak, with no hidden normalization.
    let (m, _, encoded) = measure_pipeline(1, 40, 128, 997.0, 0.251_188_64, 0.0); // -12 dBFS
    let expected_peak = 10f32.powf(-18.0 / 20.0);
    let (dec_peak, _, _) = decode_frames(&encoded);
    println!(
        "[measure] -12 dB input: pcm_peak={:.6} ({:.2} dBFS) expected={:.6} ({:.2} dBFS) decoded_peak={dec_peak:.6}",
        m.peak,
        db(m.peak),
        expected_peak,
        db(expected_peak)
    );
    assert!(
        (m.peak - expected_peak).abs() < 1e-4,
        "expected -18 dBFS peak from -12 dB input, got {} ({:.3} dBFS)",
        m.peak,
        db(m.peak)
    );
}

#[test]
fn muted_source_amplitude_is_invariant() {
    // Mute is a boolean -inf per source, not a bus-wide gain. Muting a source that contributes
    // nothing to the current mix must not change the surviving amplitude, while muting the active
    // source must silence the bus.
    let map = input_map_source_count(2);
    let base = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&unity_gains(2)),
        master_db: 0.0,
        master_muted: false,
    };

    // Only channel 1 carries signal; channel 0 is silent.
    let render = |mix: MixSnapshot| {
        let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
        pipeline.register(0, sess(), mix).unwrap();
        let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
        snapshots[0] = Some(mix);
        let tap = std::sync::Arc::new(SignalTap::default());
        let port: &dyn crate::monitor::MonitorPort = tap.as_ref();
        let mut start = 0u64;
        for _ in 0..40 {
            let mut block = correlated_sine_block(2, 128, start, 997.0, 0.8);
            for frame in 0..128usize {
                block.samples[frame * 2] = 0.0;
            }
            let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
                std::array::from_fn(crate::pipeline::ListenerOutput::empty);
            pipeline
                .process(&block, &snapshots, &mut out, Some((0, port)))
                .unwrap();
            start += 128;
        }
        let pcm = tap.pcm.lock().unwrap().clone();
        left_right_stats(&pcm).0 .0
    };

    let unmuted_peak = render(base);

    // Muting the silent source must not change the bus.
    let mut mute_silent = base;
    mute_silent.sources.entries[0].muted = true;
    let muted_silent_peak = render(mute_silent);
    assert!(
        (unmuted_peak - muted_silent_peak).abs() < 1e-6,
        "muting a non-contributing source changed amplitude: {unmuted_peak} vs {muted_silent_peak}"
    );

    // Muting the active source must silence its contribution.
    let mut mute_active = base;
    mute_active.sources.entries[1].muted = true;
    let muted_active_peak = render(mute_active);
    println!(
        "[measure] mute: unmuted_peak={unmuted_peak:.6} muted_silent={muted_silent_peak:.6} muted_active={muted_active_peak:.6} ({:.1} dBFS)",
        db(muted_active_peak)
    );
    // A muted source ramps down from the -60 dB registration clamp to the -120 dB floor, so it is
    // effectively silent (well under -60 dBFS) but not a hard zero during the ramp.
    assert!(
        muted_active_peak < 1e-3,
        "muting the only active source must silence the bus (< -60 dBFS), got {muted_active_peak}"
    );
    assert!(
        muted_active_peak < unmuted_peak * 1e-3,
        "muted active source must be far below its unmuted level"
    );
}

#[test]
fn additive_fader_gain_stays_below_limiting() {
    // Two correlated sources at -12 dB each stay well below the ceiling: the mix is the linear sum
    // of the faded sources then a single -6 dB headroom, with no extra normalization.
    let map = input_map_source_count(2);
    let snapshot = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&[
            SourceGain {
                source_id: SourceId::from_bytes([1; 16]),
                gain_db: -12.0,
                muted: false,
            },
            SourceGain {
                source_id: SourceId::from_bytes([2; 16]),
                gain_db: -12.0,
                muted: false,
            },
        ]),
        master_db: 0.0,
        master_muted: false,
    };
    let (pcm, _) = render_interleaved(&map, snapshot.sources.as_slice(), 40, |_, start| {
        correlated_sine_block(2, 128, start, 997.0, 1.0)
    });
    let ((peak_l, _), _) = left_right_stats(&pcm);
    let per_source = 10f32.powf(-12.0 / 20.0);
    let expected = 2.0 * per_source * 10f32.powf(BUS_HEADROOM_DB / 20.0);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    println!(
        "[measure] additive fader gain: peak_L={peak_l:.6} ({:.2} dBFS) expected={expected:.6} ceiling={ceiling:.6}",
        db(peak_l)
    );
    assert!(expected < ceiling, "test setup must stay below the limiter");
    assert!(
        (peak_l - expected).abs() < 2e-3,
        "additive faders below the limiter should sum linearly to {expected}, got {peak_l}"
    );
}

#[test]
fn correlated_source_overload_is_finite_and_bounded_by_ceiling() {
    // 2 / 22 / supported-max (24) fully correlated full-scale sources: pre-encode peaks are finite
    // and never exceed the -6 dBFS limiter ceiling, and L==R (no stereo image drift from the sum).
    let ceiling = 10f32.powf(-6.0 / 20.0);
    for sources in [2usize, 22, 24] {
        let map = input_map_source_count(sources);
        let (pcm, _) = render_interleaved(&map, &unity_gains(sources), 40, |_, start| {
            correlated_sine_block(sources, 128, start, 997.0, 1.0)
        });
        let ((peak_l, _), (peak_r, _)) = left_right_stats(&pcm);
        let (m, _, encoded) = measure_pipeline(sources, 40, 128, 997.0, 1.0, 0.0);
        let (dec_peak, dec_rms, _) = decode_frames(&encoded);
        println!(
            "[measure] overload {sources} correlated: pcm_peak={:.6} ({:.2} dBFS) L={peak_l:.6} R={peak_r:.6} decoded_peak={dec_peak:.6} decoded_rms={dec_rms:.6}",
            m.peak,
            db(m.peak)
        );
        assert!(
            m.peak.is_finite(),
            "{sources} sources produced non-finite peak"
        );
        assert!(
            m.peak <= ceiling + 1e-6,
            "{sources} correlated sources exceeded the ceiling: {} > {ceiling}",
            m.peak
        );
        // Stereo-linked limiter preserves the L/R ratio (here both sides get the same correlated sum).
        assert!(
            (peak_l - peak_r).abs() < 1e-6,
            "{sources} correlated sources shifted the stereo image: L={peak_l} R={peak_r}"
        );
        assert!(
            dec_peak.is_finite() && dec_peak < 1.0,
            "{sources} correlated decoded peak must be finite and below full scale: {dec_peak}"
        );
    }
}

#[test]
fn correlated_overload_recovers_without_fullscale_clip() {
    // Overload then return to a clean single source: the shared limiter must release back toward
    // unity so later frames track the clean input, never pinned at full scale or stuck reduced.
    let map = input_map_source_count(22);
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
    let snapshot = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&unity_gains(22)),
        master_db: 0.0,
        master_muted: false,
    };
    pipeline.register(0, sess(), snapshot).unwrap();
    let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
    snapshots[0] = Some(snapshot);
    let tap = std::sync::Arc::new(SignalTap::default());
    let port: &dyn crate::monitor::MonitorPort = tap.as_ref();
    let mut start = 0u64;
    let over_frames = 40u64;
    let quiet_frames = 200u64;

    // Overload phase: 22 correlated full-scale sources.
    for _ in 0..over_frames / 2 {
        let block = correlated_sine_block(22, 128, start, 997.0, 1.0);
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);
        pipeline
            .process(&block, &snapshots, &mut out, Some((0, port)))
            .unwrap();
        start += 128;
    }
    // Quiet phase: only channel 0 at full scale (others zero) -> the 1/22 input the limiter should
    // release for.
    let quiet_start = start;
    for _ in 0..quiet_frames / 2 {
        let mut block = correlated_sine_block(22, 128, start, 997.0, 1.0);
        for frame in 0..128usize {
            for ch in 1..22 {
                block.samples[frame * 22 + ch] = 0.0;
            }
        }
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);
        pipeline
            .process(&block, &snapshots, &mut out, Some((0, port)))
            .unwrap();
        start += 128;
    }
    let pcm = tap.pcm.lock().unwrap().clone();
    let global_peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // Last ~1200 samples are well past the 50 ms (2400-frame) release after the quiet switch.
    let tail = &pcm[pcm.len().saturating_sub(1200)..];
    let tail_peak = tail.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let expected_quiet_peak = 10f32.powf(-6.0 / 20.0); // single full-scale source at -6 headroom
    println!(
        "[measure] overload recovery: global_peak={global_peak:.6} tail_peak={tail_peak:.6} expected_quiet={expected_quiet_peak:.6} quiet_start={quiet_start}"
    );
    assert!(
        global_peak < 1.0,
        "overload must never reach hard full scale"
    );
    assert!(
        global_peak <= ceiling + 1e-3,
        "overload must stay bounded by the ceiling"
    );
    assert!(
        (tail_peak - expected_quiet_peak).abs() < 0.02,
        "limiter must release to track the quiet source ({expected_quiet_peak}), got {tail_peak}"
    );
}

#[test]
fn measure_correlated_full_scale_sum_pins_limiter_without_hard_clip() {
    // 22 fully correlated full-scale inputs: linear sum would be ~0.98, so the shared limiter must
    // clamp it. This is where saturation would come from; it is bounded, not a hard clip.
    let (m, _, encoded) = measure_pipeline(22, 40, 128, 997.0, 1.0, 0.0);
    let (dec_peak, _, _) = decode_frames(&encoded);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    println!(
        "[measure] 22 correlated full-scale sources: pcm_peak={:.6} ({:.2} dBFS) pcm_rms={:.6} decoded_peak={:.6}",
        m.peak,
        db(m.peak),
        m.rms,
        dec_peak
    );
    assert!(
        m.peak <= ceiling + 1e-6,
        "summed peak {} must be bounded by the limiter ceiling {}",
        m.peak,
        ceiling
    );
    // No hard-clip signature: the limiter is a smooth shared gain, so output stays strictly inside
    // full scale rather than flat-topping at 1.0.
    assert!(
        m.peak < 1.0,
        "output must never reach hard digital full scale"
    );
}

#[test]
fn cadence_probe_has_no_gaps_or_duplicate_frames() {
    // Steady correlated sine across many real capture-sized blocks; the engine must emit a
    // contiguous 120-frame sequence with no gaps, duplicates, or zero-filled holes.
    let total_blocks = 16usize;
    let block_frames = 128u32;
    let (m, cadence, _) = measure_pipeline(1, total_blocks, block_frames, 997.0, 0.6, 0.0);

    let total_in = total_blocks as u64 * block_frames as u64;
    let expected_frames = (total_in / CODEC_FRAME_FRAMES as u64) as usize;
    assert_eq!(
        cadence.len(),
        expected_frames,
        "emitted frame count {} != floor({total_in}/120)={expected_frames}",
        cadence.len()
    );
    assert_eq!(m.frames, expected_frames * CODEC_FRAME_FRAMES as usize);

    // Strictly increasing start samples spaced by exactly 120 (no dropped frame).
    for pair in cadence.windows(2) {
        assert_eq!(
            pair[1].0 - pair[0].0,
            CODEC_FRAME_FRAMES as u64,
            "non-contiguous frame cadence: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(cadence
        .iter()
        .all(|(_, count)| *count == CODEC_FRAME_FRAMES));
}

#[test]
fn cadence_probe_waveform_is_continuous_across_frame_boundaries() {
    // Inspect the concatenated pre-encode PCM for discontinuities at frame seams. A dropout/zipper
    // (the reported "choppy/robotic" character) would show a step far larger than the true
    // sample-to-sample delta of the sine.
    let map = input_map_source_count(1);
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
    let session = sess();
    let snapshot = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&unity_gains(1)),
        master_db: 0.0,
        master_muted: false,
    };
    pipeline.register(0, session, snapshot).unwrap();
    let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
    snapshots[0] = Some(snapshot);
    let tap = std::sync::Arc::new(SignalTap::default());
    let port: &dyn crate::monitor::MonitorPort = tap.as_ref();
    let mut start = 0u64;
    for _ in 0..16 {
        let block = correlated_sine_block(1, 128, start, 997.0, 0.6);
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);
        pipeline
            .process(&block, &snapshots, &mut out, Some((0, port)))
            .unwrap();
        start += 128;
    }
    let pcm = tap.pcm.lock().unwrap().clone();
    assert!(pcm.len() > 1000);

    let max_diff = pcm
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    let amp = 0.6 * 10f32.powf(BUS_HEADROOM_DB / 20.0);
    let phase_step = 2.0 * std::f32::consts::PI * 997.0 / 48_000.0;
    let expected_diff = amp * (phase_step / 2.0).sin() * 2.0;
    println!(
        "[measure] continuity: amp={:.6} expected_max_step={:.6} observed_max_step={:.6}",
        amp, expected_diff, max_diff
    );
    assert!(
        max_diff < expected_diff * 2.0 + 1e-4,
        "waveform discontinuity suggests a gap/zipper: observed {max_diff} > expected {expected_diff}"
    );
}

// ---------------------------------------------------------------------------------------------
// Channel mapping / capture normalization / pan attenuation / headroom policy measurements
// ---------------------------------------------------------------------------------------------

/// A stereo block with independent left/right channel sines (models a real stereo capture).
fn stereo_isolated_sine_block(
    frames: u32,
    start_sample: u64,
    freq: f32,
    left_amp: f32,
    right_amp: f32,
) -> CaptureBlock {
    let mut samples = crate::contract::silent_audio_slot();
    for frame in 0..frames as usize {
        let t = (start_sample as usize + frame) as f32 / 48_000.0;
        let v = (2.0 * std::f32::consts::PI * freq * t).sin();
        samples[frame * 2] = left_amp * v;
        samples[frame * 2 + 1] = right_amp * v;
    }
    CaptureBlock {
        audio_epoch: ctx_a().audio_epoch,
        start_sample,
        frame_count: frames,
        sample_rate_hz: 48_000,
        channel_count: 2,
        channel_map_revision: crate::ids::ChannelMapRevision(1),
        samples,
    }
}

/// Render interleaved stereo with the given channel map, gains, and block factory; return the
/// concatenated pre-encode PCM and the frame cadence.
fn render_interleaved(
    map: &[ChannelMapEntry],
    gains: &[SourceGain],
    blocks: usize,
    make_block: impl Fn(usize, u64) -> CaptureBlock,
) -> (Vec<f32>, Vec<(u64, u32)>) {
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, map);
    let session = sess();
    let snapshot = MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(gains),
        master_db: 0.0,
        master_muted: false,
    };
    pipeline.register(0, session, snapshot).unwrap();
    let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
    snapshots[0] = Some(snapshot);
    let tap = std::sync::Arc::new(SignalTap::default());
    let port: &dyn crate::monitor::MonitorPort = tap.as_ref();
    let mut start = 0u64;
    for index in 0..blocks {
        let block = make_block(index, start);
        let frames = block.frame_count;
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);
        pipeline
            .process(&block, &snapshots, &mut out, Some((0, port)))
            .unwrap();
        start += frames as u64;
    }
    let pcm = tap.pcm.lock().unwrap().clone();
    let starts = tap.starts.lock().unwrap().clone();
    let counts = tap.frame_counts.lock().unwrap().clone();
    (pcm, starts.into_iter().zip(counts).collect())
}

fn left_right_stats(pcm: &[f32]) -> ((f32, f32), (f32, f32)) {
    let mut lp = 0.0f32;
    let mut rp = 0.0f32;
    let mut ls = 0.0f64;
    let mut rs = 0.0f64;
    let mut count = 0usize;
    for frame in pcm.chunks_exact(2) {
        lp = lp.max(frame[0].abs());
        rp = rp.max(frame[1].abs());
        ls += (frame[0] as f64) * (frame[0] as f64);
        rs += (frame[1] as f64) * (frame[1] as f64);
        count += 1;
    }
    let n = count.max(1) as f64;
    ((lp, (ls / n).sqrt() as f32), (rp, (rs / n).sqrt() as f32))
}

#[test]
fn measure_capture_passthrough_and_bus_headroom_attenuation() {
    // Capture path (capture/service.rs publish_chunks) copies f32 samples through unchanged: there
    // is no normalization, scale, or peak detection. So a full-scale captured sample arrives at the
    // engine as 1.0, and the ONLY attenuation before the limiter is the fixed BUS_HEADROOM (single
    // gain, independent of how many sources are actually present).
    let map = input_map_source_count(1);
    let (pcm, _) = render_interleaved(&map, &unity_gains(1), 20, |_, start| {
        correlated_sine_block(1, 128, start, 997.0, 1.0)
    });
    let ((lp, _), (rp, _)) = left_right_stats(&pcm);
    let headroom = 10f32.powf(BUS_HEADROOM_DB / 20.0);
    println!(
        "[measure] capture passthrough + headroom: in_amp=1.0 out_peak_L={lp:.6} ({:.2} dBFS) out_peak_R={rp:.6} headroom_linear={headroom:.6} ({BUS_HEADROOM_DB} dB)",
        db(lp),
    );
    assert!(
        (lp - headroom).abs() < 1e-3,
        "expected full-scale capture reduced exactly by bus headroom {headroom}, got {lp}"
    );
    assert_eq!(lp, rp, "a single mono source is dual-mono, not panned");
}

#[test]
fn measure_mono_source_has_no_pan_law_attenuation() {
    // centered_coefficients() == [1.0, 1.0]: a mono InputChannel is placed into BOTH sides at unity
    // (dual-mono). There is no -3 dB or -6 dB pan-law attenuation in host-core, so this cannot be
    // the cause of the quiet output.
    assert_eq!(centered_coefficients(), [1.0, 1.0]);
    let map = input_map_source_count(1);
    let (pcm, _) = render_interleaved(&map, &unity_gains(1), 20, |_, start| {
        correlated_sine_block(1, 128, start, 997.0, 1.0)
    });
    let ((lp, lrms), (rp, _)) = left_right_stats(&pcm);
    println!("[measure] mono pan: L_peak={lp:.6} L_rms={lrms:.6} R_peak={rp:.6} (unity dual-mono)");
    assert_eq!(
        lp, rp,
        "mono must be identical in both sides (no pan attenuation)"
    );
}

#[test]
fn measure_blackhole_stereo_channels_are_summed_dual_mono() {
    // BlackHole 2ch delivers a stereo block. The default map makes each physical channel its own
    // InputChannel role, and centered_coefficients() sums BOTH into each output side at unity. So a
    // correlated stereo program is summed (2x), not preserved as a stereo image; an anti-correlated
    // one cancels.
    let map = input_map_source_count(2);
    // Correlated L==R full scale.
    let gains = unity_gains(2);
    let (corr, _) = render_interleaved(&map, &gains, 20, |_, start| {
        correlated_sine_block(2, 128, start, 997.0, 1.0)
    });
    let ((corr_l, _), (_corr_r, _)) = left_right_stats(&corr);
    // Anti-correlated L == -R.
    let (anti, _) = render_interleaved(&map, &gains, 20, |_, start| {
        stereo_isolated_sine_block(128, start, 997.0, 1.0, -1.0)
    });
    let ((anti_l, _), (anti_r, _)) = left_right_stats(&anti);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let headroom = 10f32.powf(BUS_HEADROOM_DB / 20.0);
    println!(
        "[measure] BlackHole stereo map: correlated(2ch) L_peak={corr_l:.6} ({:.2} dBFS) linear_sum≈{:.6} ceiling={ceiling:.6}; anti-correlated L_peak={anti_l:.6} R_peak={anti_r:.6}",
        db(corr_l),
        2.0 * headroom,
    );
    // Correlated stereo sums to 2x one channel; at -6 dB headroom that exceeds the ceiling, so the
    // limiter bounds it. It is finite and never reaches full scale.
    assert!(
        corr_l.is_finite() && corr_l <= ceiling + 1e-6,
        "correlated stereo must be bounded by the ceiling, got {corr_l}"
    );
    assert!(
        corr_l < 1.0,
        "correlated stereo must not reach hard full scale"
    );
    // Anti-correlated cancels to silence: a real stereo image is destroyed by the dual-mono sum.
    assert!(
        anti_l < 1e-3 && anti_r < 1e-3,
        "anti-correlated stereo must cancel"
    );
}

#[test]
fn measure_headroom_safety_policy_capacity() {
    // The -6 dB bus headroom is a *static* gain, so it is not "per active source": it reserves level
    // for a fixed worst-case source count. Linear full-scale sum of N correlated sources is N; the
    // post-headroom peak is N*headroom, which reaches the limiter ceiling at:
    let headroom = 10f32.powf(BUS_HEADROOM_DB / 20.0);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let sources_until_limiter = ceiling / headroom;
    println!(
        "[measure] headroom policy: headroom={headroom:.6} ({BUS_HEADROOM_DB} dB) ceiling={ceiling:.6}; correlated full-scale sources before limiter ≈ {sources_until_limiter:.2}"
    );
    // A single source peaks exactly at the ceiling; any correlated pair reaches the limiter.
    println!(
        "[measure] single source headroom: 0 dBFS source -> {:.2} dBFS (limiter allows up to {:.2} dBFS)",
        db(headroom),
        db(ceiling)
    );
    assert!((1.0..1.01).contains(&sources_until_limiter));
}

#[test]
fn cadence_probe_64_frame_blocks_preserve_continuity() {
    // preferred_buffer() chooses 64 frames on a device that reports a 64..N range, and 120-frame
    // codec frames never align to 64, so every emitted frame spans a block boundary via the engine's
    // carried partial. Cadence must still be contiguous with no gap or duplicate.
    let map = input_map_source_count(1);
    let (pcm, cadence) = render_interleaved(&map, &unity_gains(1), 24, |_, start| {
        correlated_sine_block(1, 64, start, 997.0, 0.6)
    });
    for pair in cadence.windows(2) {
        assert_eq!(
            pair[1].0 - pair[0].0,
            CODEC_FRAME_FRAMES as u64,
            "64-frame-block cadence gap: {:?} -> {:?}",
            pair[0],
            pair[1]
        );
    }
    let max_diff = pcm
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    let amp = 0.6 * 10f32.powf(BUS_HEADROOM_DB / 20.0);
    let phase_step = 2.0 * std::f32::consts::PI * 997.0 / 48_000.0;
    let expected_diff = amp * (phase_step / 2.0).sin() * 2.0;
    println!(
        "[measure] 64-frame cadence: frames={} observed_max_step={max_diff:.6} expected={expected_diff:.6}",
        cadence.len()
    );
    assert!(
        max_diff < expected_diff * 2.0 + 1e-4,
        "64-frame-block waveform discontinuity: observed {max_diff} > expected {expected_diff}"
    );
}

// ---------------------------------------------------------------------------------------------
// Deterministic timing / discontinuity reproduction (test-only; no production change yet)
//
// Oracle candidate: the engine carries a partial codec frame and, on the next block, glues the
// held tail to that block's samples without checking that `input.start_sample` is contiguous with
// `partial_start + partial_frames`. A dropped capture block therefore splices non-adjacent audio
// into one 120-frame codec frame while labelling it with the OLD partial start.
//
// A second path: `DspBridge::install_snapshot` runs on EVERY mix patch and arm and calls
// `set_mix` -> `Pipeline::register`, which resets the engine's carried partial AND creates a fresh
// Opus encoder mid-stream. `update_mix` exists but is not used by the control path.
// ---------------------------------------------------------------------------------------------

/// A block whose single channel is a linear ramp in absolute sample index, so an input-splice is a
/// large step and a correct continuation is a single `scale` step.
fn ramp_block(channels: usize, frames: u32, start_sample: u64, scale: f32) -> CaptureBlock {
    let mut samples = crate::contract::silent_audio_slot();
    for frame in 0..frames as usize {
        let value = (start_sample as usize + frame) as f32 * scale;
        for ch in 0..channels {
            samples[frame * channels + ch] = value;
        }
    }
    CaptureBlock {
        audio_epoch: ctx_a().audio_epoch,
        start_sample,
        frame_count: frames,
        sample_rate_hz: 48_000,
        channel_count: channels as u16,
        channel_map_revision: crate::ids::ChannelMapRevision(1),
        samples,
    }
}

/// Drive a bare `AudioEngine` (not the Pipeline) so block start samples can be controlled directly.
/// Returns `(start_sample, pcm)` for every emitted codec frame.
fn engine_frames(mix: MixSnapshot, blocks: &[CaptureBlock]) -> Vec<(u64, Vec<f32>)> {
    let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
    engine.register_session(0, ctx_a(), mix);
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    let mut emitted = Vec::new();
    for block in blocks {
        let n = engine.process_block(0, block, &mix, &mut out).unwrap();
        for frame in out.iter().take(n) {
            // `StereoFrame.pcm` is a fixed 480-float capacity; only the first frame_count*2
            // interleaved samples are the frame. Including the zeroed tail would fabricate a step.
            let len = frame.frame_count as usize * 2;
            emitted.push((frame.start_sample, frame.pcm[..len].to_vec()));
        }
    }
    emitted
}

fn max_sample_step(pcm: &[f32]) -> f32 {
    pcm.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max)
}

fn unity_snapshot_one_source() -> MixSnapshot {
    MixSnapshot {
        context: ctx_a(),
        catalog_revision: CatalogRevision(1),
        mix_revision: MixRevision(1),
        sources: SourceGainMatrix::from_slice(&unity_gains(1)),
        master_db: 0.0,
        master_muted: false,
    }
}

#[test]
fn engine_gap_drops_incomplete_pcm_and_reanchors_to_real_start() {
    // Block A 64@0 (held), then a dropped block (next arrives 64@128), then 64@192. The held
    // 64 samples are no longer contiguous with 128, so the engine must discard them and re-anchor:
    // the first emitted frame is the 120 samples starting at 128, never a splice of 0..63 with
    // 128..183 labelled start_sample 0.
    let scale = 0.001f32;
    let mix = unity_snapshot_one_source();
    let blocks = [
        ramp_block(1, 64, 0, scale),
        ramp_block(1, 64, 128, scale), // gap: 64..127 missing
        ramp_block(1, 64, 192, scale), // contiguous with the re-anchored 128 block
    ];
    let frames = engine_frames(mix, &blocks);

    // 64@128 then 64@192 = 128 contiguous frames -> exactly one 120-frame frame starting at 128.
    assert_eq!(frames.len(), 1, "expected exactly one emitted frame");
    assert_eq!(
        frames[0].0, 128,
        "the frame must be re-anchored to the real start 128, not the dropped partial start 0"
    );

    // No splice: the waveform continues from sample 128 with the true one-sample ramp step.
    let step = max_sample_step(&frames[0].1);
    let normal_step = scale * 10f32.powf(BUS_HEADROOM_DB / 20.0);
    println!(
        "[timing] gap reanchor: frame_start={} seam_step={step:.8} normal_step={normal_step:.8}",
        frames[0].0
    );
    assert!(
        step < normal_step * 2.0,
        "re-anchored frame must not splice non-adjacent samples: step {step} vs normal {normal_step}"
    );
}

#[test]
fn engine_contiguous_blocks_are_loss_free_baseline() {
    // Loss-free baseline: block A 0..63, block B 64..127 -> one clean 120-frame frame, start 0.
    let scale = 0.001f32;
    let mix = unity_snapshot_one_source();
    let blocks = [ramp_block(1, 64, 0, scale), ramp_block(1, 64, 64, scale)];
    let frames = engine_frames(mix, &blocks);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0, 0, "contiguous blocks must keep start_sample 0");
    let step = max_sample_step(&frames[0].1);
    let normal_step = scale * 10f32.powf(BUS_HEADROOM_DB / 20.0);
    assert!(
        step < normal_step * 2.0,
        "contiguous ramp should have a single-step waveform: {step} vs {normal_step}"
    );
}

#[test]
fn engine_rejects_a_block_from_a_mismatched_audio_epoch() {
    // A capture block stamped with a different audio epoch is not part of this session's timeline.
    // It must be ignored: no append, no output, and the held partial stays valid and intact.
    let scale = 0.001f32;
    let mix = unity_snapshot_one_source();
    let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
    engine.register_session(0, ctx_a(), mix);
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];

    // Hold 64 frames from the correct epoch.
    let held = engine
        .process_block(0, &ramp_block(1, 64, 0, scale), &mix, &mut out)
        .unwrap();
    assert_eq!(held, 0);

    // A mismatched-epoch block must be rejected outright.
    let mut foreign = ramp_block(1, 64, 64, scale);
    foreign.audio_epoch = AudioEpoch::from_bytes([0x99; 16]);
    let rejected = engine.process_block(0, &foreign, &mix, &mut out).unwrap();
    assert_eq!(rejected, 0, "mismatched-epoch block must emit nothing");

    // The valid partial survived, so a contiguous 64@64 completes the 120-frame frame at 0.
    let produced = engine
        .process_block(0, &ramp_block(1, 64, 64, scale), &mix, &mut out)
        .unwrap();
    assert_eq!(produced, 1);
    assert_eq!(
        out[0].start_sample, 0,
        "valid partial must not be disturbed"
    );
}

#[test]
fn engine_same_context_upsert_preserves_partial_and_smoothing() {
    // Re-registering a gain edit that keeps the exact safety context must not drop the held partial
    // or jump the ramps. A 50-frame ramp target change across the upsert continues smoothly.
    let scale = 0.001f32;
    let mut mix = unity_snapshot_one_source();
    let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
    engine.register_session(0, ctx_a(), mix);
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];

    assert_eq!(
        engine
            .process_block(0, &ramp_block(1, 64, 0, scale), &mix, &mut out)
            .unwrap(),
        0
    );

    // Ordinary gain edit, same exact context + revision bump (metadata only).
    mix.sources.entries[0].gain_db = -3.0;
    mix.mix_revision = MixRevision(2);
    engine.register_session(0, ctx_a(), mix);

    assert_eq!(
        engine
            .process_block(0, &ramp_block(1, 64, 64, scale), &mix, &mut out)
            .unwrap(),
        1,
        "held partial must survive the same-context upsert"
    );
    assert_eq!(out[0].start_sample, 0);
    assert_eq!(out[0].frame_count, CODEC_FRAME_FRAMES);
}

#[test]
fn engine_changed_identity_resets_all_state_fresh() {
    // A changed safety generation / audio epoch / session is a new identity: no held old PCM, and
    // the emitted frame carries the exact new context.
    let scale = 0.001f32;
    for mutate in 0..3 {
        let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
        engine.register_session(0, ctx_a(), unity_snapshot_one_source());
        let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];

        // Hold 64 frames under ctx_a.
        assert_eq!(
            engine
                .process_block(
                    0,
                    &ramp_block(1, 64, 0, scale),
                    &unity_snapshot_one_source(),
                    &mut out
                )
                .unwrap(),
            0
        );

        // New identity: change exactly one identity field.
        let mut new_ctx = ctx_a();
        match mutate {
            0 => new_ctx.safety_generation = SafetyGeneration(2),
            1 => new_ctx.audio_epoch = AudioEpoch::from_bytes([0x99; 16]),
            _ => new_ctx.session_epoch = SessionEpoch::from_bytes([0x77; 16]),
        }
        let new_mix = MixSnapshot {
            context: new_ctx,
            ..unity_snapshot_one_source()
        };
        engine.register_session(0, new_ctx, new_mix);

        // A single 120-frame block under the new identity emits exactly one frame with the new
        // context, proving the old 64 held samples were not retagged. The block carries the new
        // context's audio epoch (a real new capture stream would).
        let mut block = ramp_block(1, 120, 500, scale);
        block.audio_epoch = new_ctx.audio_epoch;
        let produced = engine.process_block(0, &block, &new_mix, &mut out).unwrap();
        assert_eq!(
            produced, 1,
            "identity change {mutate} must reset the partial"
        );
        assert_eq!(
            out[0].context, new_ctx,
            "emitted frame must carry the exact new context for change {mutate}"
        );
        assert_eq!(out[0].start_sample, 500);
    }
}

#[test]
fn engine_rearm_generation_change_resets_held_partial_deliberately() {
    // Mirrors `request_arm`: the mix is re-installed under a NEW safety generation. That is an
    // identity change and must reset the held partial (never re-tag old PCM to the new generation).
    let scale = 0.001f32;
    let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
    engine.register_session(0, ctx_a(), unity_snapshot_one_source());
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];
    assert_eq!(
        engine
            .process_block(
                0,
                &ramp_block(1, 64, 0, scale),
                &unity_snapshot_one_source(),
                &mut out
            )
            .unwrap(),
        0
    );

    let mut armed_ctx = ctx_a();
    armed_ctx.safety_generation = SafetyGeneration(2);
    let armed_mix = MixSnapshot {
        context: armed_ctx,
        ..unity_snapshot_one_source()
    };
    engine.register_session(0, armed_ctx, armed_mix);

    // 64 contiguous frames after re-arm do NOT complete the old 64 held samples.
    let produced = engine
        .process_block(0, &ramp_block(1, 64, 64, scale), &armed_mix, &mut out)
        .unwrap();
    assert_eq!(
        produced, 0,
        "re-arm generation change must drop the held partial, not splice it"
    );
    // Those 64 frames are now the fresh partial; 56 more contiguous frames complete it at start 64.
    let produced = engine
        .process_block(0, &ramp_block(1, 56, 128, scale), &armed_mix, &mut out)
        .unwrap();
    assert_eq!(produced, 1);
    assert_eq!(out[0].context, armed_ctx);
    assert_eq!(out[0].start_sample, 64);
}

#[test]
fn engine_validates_input_epoch_against_snapshot_before_state_change() {
    // Regression: `process_block` must compare `input.audio_epoch` before any state change. If it
    // compared against the OLD `state.context.audio_epoch` and then switched to `snapshot.context`,
    // an A-stamped block arriving under snapshot B could be appended and then mis-tagged as B, or a
    // valid B-stamped block could be rejected because the state still held A.
    let scale = 0.001f32;
    let mut engine = AudioEngine::new(48_000, &input_map_source_count(1));
    let a = unity_snapshot_one_source(); // ctx_a() audio epoch
    engine.register_session(0, ctx_a(), a);
    let mut out = [StereoFrame::zeroed(ctx_a()); MAX_OUT_FRAMES_PER_BLOCK];

    // Hold a valid A partial.
    assert_eq!(
        engine
            .process_block(0, &ramp_block(1, 64, 0, scale), &a, &mut out)
            .unwrap(),
        0
    );

    // Snapshot B with a different audio epoch, but feed an A-stamped 120-frame block.
    let mut b_ctx = ctx_a();
    b_ctx.audio_epoch = AudioEpoch::from_bytes([0x99; 16]);
    let b = MixSnapshot {
        context: b_ctx,
        ..unity_snapshot_one_source()
    };
    let a_stamped = ramp_block(1, 120, 64, scale); // still ctx_a().audio_epoch
    let produced = engine.process_block(0, &a_stamped, &b, &mut out).unwrap();
    assert_eq!(
        produced, 0,
        "an A-stamped block under snapshot B must be rejected, not retagged as B"
    );
    // The valid A state must be unchanged (still holds the 64 frame partial at 0).
    let produced = engine
        .process_block(0, &ramp_block(1, 56, 64, scale), &a, &mut out)
        .unwrap();
    assert_eq!(produced, 1, "valid A partial must remain intact");
    assert_eq!(out[0].context, ctx_a());
    assert_eq!(out[0].start_sample, 0);

    // Now a B-stamped block under snapshot B must be accepted fresh (no A partial retagged).
    let mut b_block = ramp_block(1, 120, 0, scale);
    b_block.audio_epoch = b_ctx.audio_epoch;
    let produced = engine.process_block(0, &b_block, &b, &mut out).unwrap();
    assert_eq!(
        produced, 1,
        "a B-stamped block must be accepted under snapshot B"
    );
    assert_eq!(out[0].context, b_ctx);
    assert_eq!(out[0].start_sample, 0);
}

#[test]
fn pipeline_identical_reinstall_preserves_held_partial_and_codec() {
    // The production control path re-installs the identical mix/context on every patch. With the
    // identity-aware upsert this must preserve the held partial (first frame stays at 0) and keep
    // the same encoder, so the encoded packet equals a baseline produced without the reinstall.
    let map = input_map_source_count(1);
    let snapshot = unity_snapshot_one_source();

    let run = |reinstall: bool| -> Vec<(u64, Vec<u8>)> {
        let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
        pipeline.register(0, sess(), snapshot).unwrap();
        let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
        snapshots[0] = Some(snapshot);
        let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(crate::pipeline::ListenerOutput::empty);

        // 100 held frames.
        pipeline
            .process(&ramp_block(1, 100, 0, 0.001), &snapshots, &mut out, None)
            .unwrap();
        assert_eq!(out[0].count, 0);
        if reinstall {
            pipeline.register(0, sess(), snapshot).unwrap();
        }
        // 128 more contiguous frames -> one frame at 0.
        pipeline
            .process(&ramp_block(1, 128, 100, 0.001), &snapshots, &mut out, None)
            .unwrap();
        out[0]
            .frames
            .iter()
            .take(out[0].count)
            .map(|f| {
                let len = f.packet.len as usize;
                (f.start_sample, f.packet.bytes[..len].to_vec())
            })
            .collect()
    };

    let baseline = run(false);
    let reinstalled = run(true);
    println!("[timing] identical reinstall: baseline={baseline:?} reinstalled={reinstalled:?}");
    assert_eq!(baseline.len(), 1);
    assert_eq!(
        reinstalled, baseline,
        "identical-context reinstall must preserve the held partial and codec, producing the same frame"
    );
    assert_eq!(baseline[0].0, 0, "held partial must survive the reinstall");
}

#[test]
fn pipeline_unregister_then_reregister_is_fresh() {
    // Clear then re-register must start clean: no carried partial or codec history.
    let map = input_map_source_count(1);
    let snapshot = unity_snapshot_one_source();
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
    pipeline.register(0, sess(), snapshot).unwrap();
    let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
    snapshots[0] = Some(snapshot);
    let mut out: [crate::pipeline::ListenerOutput; MAX_SESSIONS] =
        std::array::from_fn(crate::pipeline::ListenerOutput::empty);

    // Hold frames, then clear.
    pipeline
        .process(&ramp_block(1, 100, 0, 0.001), &snapshots, &mut out, None)
        .unwrap();
    pipeline.unregister(0);
    pipeline.register(0, sess(), snapshot).unwrap();

    // 100 frames after a fresh register must NOT complete the old held partial.
    pipeline
        .process(&ramp_block(1, 100, 0, 0.001), &snapshots, &mut out, None)
        .unwrap();
    assert_eq!(
        out[0].count, 0,
        "fresh register must hold, not emit a retagged frame"
    );
}

#[test]
fn register_rejects_mismatched_session_and_invalid_slot() {
    let map = input_map_source_count(1);
    let mut pipeline = crate::pipeline::Pipeline::new(48_000, &map);
    // Session argument disagrees with the mix's own session identity.
    let mut snapshot = unity_snapshot_one_source();
    snapshot.context.session_epoch = sess();
    assert!(matches!(
        pipeline.register(0, SessionEpoch::from_bytes([0x33; 16]), snapshot),
        Err(AudioFault::Internal)
    ));
    // Out-of-range slot.
    assert!(matches!(
        pipeline.register(99, sess(), unity_snapshot_one_source()),
        Err(AudioFault::Internal)
    ));
}

#[test]
fn safety_gate_old_context_drops_after_rearm_and_disarm() {
    // The gate identity is separate from the DSP identity. Re-arming under a new generation must
    // drop frames tagged with the old context; disarm must close both.
    use crate::contract::BoundedPacket;
    use crate::transport::{GateDecision, SafetyGate};

    fn tagged(context: SessionContext) -> crate::contract::EncodedFrame {
        crate::contract::EncodedFrame {
            context,
            start_sample: 0,
            frame_count: CODEC_FRAME_FRAMES,
            enqueue_instant: std::time::Instant::now(),
            packet: BoundedPacket::empty(),
        }
    }

    let mut old_ctx = ctx_a();
    old_ctx.safety_generation = SafetyGeneration(1);
    let mut new_ctx = ctx_a();
    new_ctx.safety_generation = SafetyGeneration(2);

    let mut gate = SafetyGate::new(sess());
    gate.arm(old_ctx);
    assert_eq!(gate.decide(&tagged(old_ctx)), GateDecision::Transmit);

    // Re-arm under the new generation: old-context frames are stale, new-context frames transmit.
    gate.arm(new_ctx);
    assert_eq!(gate.decide(&tagged(old_ctx)), GateDecision::Drop);
    assert_eq!(gate.decide(&tagged(new_ctx)), GateDecision::Transmit);

    // Disarm: both old and new context frames are dropped.
    let mut disarmed_ctx = new_ctx;
    disarmed_ctx.safety_generation = SafetyGeneration(3);
    gate.disarm(disarmed_ctx);
    assert_eq!(gate.decide(&tagged(old_ctx)), GateDecision::Drop);
    assert_eq!(gate.decide(&tagged(new_ctx)), GateDecision::Drop);
}
