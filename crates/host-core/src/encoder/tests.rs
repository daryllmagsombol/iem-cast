//! Encoder unit tests.

use super::opus_worker::OpusWorker;
use super::rtp::{rtp_step_for, RtpFrameMode};
use crate::audio::CODEC_FRAME_FRAMES;
use crate::contract::{EncodedFrame, SessionContext, StereoFrame};
use crate::ids::{AudioEpoch, SafetyGeneration, SessionEpoch, STEREO_PCM_SAMPLES};

fn ctx() -> SessionContext {
    SessionContext {
        session_epoch: SessionEpoch::from_bytes([2; 16]),
        audio_epoch: AudioEpoch::from_bytes([7; 16]),
        safety_generation: SafetyGeneration(1),
    }
}

fn test_stereo_frame_120() -> StereoFrame {
    let mut frame = StereoFrame::zeroed(ctx());
    frame.start_sample = 0;
    frame.frame_count = CODEC_FRAME_FRAMES;
    for n in 0..CODEC_FRAME_FRAMES as usize {
        let value = (n as f32 / CODEC_FRAME_FRAMES as f32) * 0.5;
        frame.pcm[n * 2] = value;
        frame.pcm[n * 2 + 1] = value;
    }
    let _ = STEREO_PCM_SAMPLES;
    frame
}

#[test]
fn opus_initial_mode_is_48000_stereo_lowdelay_120_step() {
    let worker = OpusWorker::new().unwrap();
    assert_eq!(worker.rtp_timestamp_step(), 120);
    assert!(!OpusWorker::opus_version().is_empty());
}

#[test]
fn encode_fills_context_and_bounded_packet_without_alloc() {
    let mut worker = OpusWorker::new().unwrap();
    let frame = test_stereo_frame_120();
    let mut out = EncodedFrame::zeroed();
    worker.encode(&frame, &mut out).unwrap();
    assert_eq!(out.context, frame.context);
    assert!(out.packet.len > 0 && out.packet.len as usize <= 1275);
}

#[test]
fn five_ms_probe_would_step_240_but_initial_stays_120() {
    assert_eq!(rtp_step_for(RtpFrameMode::Ms2p5), 120);
    assert_eq!(rtp_step_for(RtpFrameMode::Ms5), 240);
}
