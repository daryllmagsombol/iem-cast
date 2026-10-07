//! Per-listener Opus encoder worker.
//!
//! One worker per active listener (max 2 for the POC). Encoding is allocation-free: a reused
//! interleaved input buffer (240 floats for 120 stereo frames) feeds a reused fixed output buffer;
//! `encode_vec` is never used. Initial mode is 48 kHz stereo low-delay at 128 kbps with DTX and
//! inband FEC disabled.

use std::time::Instant;

use opus::{Application, Bitrate, Channels, Encoder};

use crate::audio::CODEC_FRAME_FRAMES;
use crate::contract::{BoundedPacket, EncodeFault, EncodedFrame, StereoFrame};
use crate::ids::{BOUNDED_PACKET_BYTES, STEREO_PCM_SAMPLES};

/// Fixed interleaved input capacity: 240 stereo frames.
const INPUT_FLOATS: usize = STEREO_PCM_SAMPLES;

/// A configured Opus encoder.
pub struct OpusWorker {
    encoder: Encoder,
    lookahead: i32,
    input: [f32; INPUT_FLOATS],
    output: [u8; BOUNDED_PACKET_BYTES],
}

impl OpusWorker {
    /// Build the fixed 48 kHz stereo low-delay encoder.
    pub fn new() -> Result<Self, EncodeFault> {
        let mut encoder = Encoder::new(48_000, Channels::Stereo, Application::LowDelay)
            .map_err(|_| EncodeFault::EncoderInit)?;
        encoder
            .set_bitrate(Bitrate::Bits(128_000))
            .map_err(|_| EncodeFault::EncoderInit)?;
        encoder.set_dtx(false).map_err(|_| EncodeFault::EncoderInit)?;
        encoder
            .set_inband_fec(false)
            .map_err(|_| EncodeFault::EncoderInit)?;
        encoder
            .set_force_channels(Some(Channels::Stereo))
            .map_err(|_| EncodeFault::EncoderInit)?;
        let lookahead = encoder.get_lookahead().map_err(|_| EncodeFault::EncoderInit)?;
        Ok(Self {
            encoder,
            lookahead: lookahead.max(0),
            input: [0.0f32; INPUT_FLOATS],
            output: [0u8; BOUNDED_PACKET_BYTES],
        })
    }

    /// Encode one stereo frame into `out`, filling its context (including generation).
    pub fn encode(&mut self, frame: &StereoFrame, out: &mut EncodedFrame) -> Result<(), EncodeFault> {
        let frames = (frame.frame_count as usize).min(CODEC_FRAME_FRAMES as usize);
        if frames == 0 {
            return Err(EncodeFault::EncodeFailed);
        }
        for n in 0..frames {
            self.input[n * 2] = frame.pcm[n * 2];
            self.input[n * 2 + 1] = frame.pcm[n * 2 + 1];
        }
        let len = self
            .encoder
            .encode_float(&self.input[..frames * 2], &mut self.output)
            .map_err(|_| EncodeFault::EncodeFailed)?;
        if len == 0 || len > self.output.len() {
            return Err(EncodeFault::EncodeFailed);
        }

        let mut packet = BoundedPacket::empty();
        packet.bytes[..len].copy_from_slice(&self.output[..len]);
        packet.len = len as u16;

        out.context = frame.context;
        out.start_sample = frame.start_sample;
        out.frame_count = frame.frame_count;
        out.enqueue_instant = Instant::now();
        out.packet = packet;
        Ok(())
    }

    /// RTP timestamp step for the initial 2.5 ms mode.
    pub fn rtp_timestamp_step(&self) -> u32 {
        CODEC_FRAME_FRAMES
    }

    /// Encoder lookahead in samples at 48 kHz.
    pub fn lookahead_samples(&self) -> u32 {
        self.lookahead.max(0) as u32
    }

    /// The linked libopus version string.
    pub fn opus_version() -> &'static str {
        opus::version()
    }
}
