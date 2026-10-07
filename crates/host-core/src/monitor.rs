//! Local monitor output: play a selected personal mix on the host's own speakers/headphones.
//!
//! This is the operator's own listening path — *not* a phone client and *not* a mixer return. The
//! monitored mix is the exact stereo the DSP already produced, so the operator hears what is being
//! cast without a second mix engine.
//!
//! ## Feedback safety
//!
//! Opening an output on the host is the one thing that can create an acoustic feedback loop with
//! live microphones. This module therefore:
//! - is **off by default** and only runs when the operator explicitly attaches it;
//! - never writes back to the mixer (capture stays input-only; there is no USB return path);
//! - drops frames rather than blocking the DSP thread when the output is behind.
//!
//! Physical feedback is still possible if the host speakers are audible to live mics; that is an
//! operator concern the UI must state, not something software can guarantee.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::CODEC_FRAME_FRAMES;
use crate::contract::StereoFrame;

/// Interleaved stereo sample capacity of one monitor chunk (one codec frame).
const MONITOR_CHUNK_SAMPLES: usize = CODEC_FRAME_FRAMES as usize * 2;
/// Bounded queue depth; a slowed output drops frames instead of stalling the DSP worker.
const MONITOR_QUEUE_CHUNKS: usize = 16;

/// A local monitor could not be started.
#[derive(Debug, thiserror::Error)]
pub enum MonitorFault {
    /// No output device matched (or none exists).
    #[error("no output device available")]
    NoOutputDevice,
    /// The device rejected the requested/default configuration.
    #[error("output configuration unsupported")]
    UnsupportedConfig,
    /// The output stream failed to build or start.
    #[error("output stream error")]
    StreamError,
}

/// Receives the monitored mix's stereo frames on the DSP worker thread.
///
/// Implementations must not block or allocate unboundedly: this runs on the worker, and a slow
/// monitor must never stall capture or the other listeners.
pub trait MonitorPort: Send + Sync + 'static {
    /// Deliver the monitored mix for one processed capture block.
    fn on_frames(&self, frames: &[StereoFrame]);
}

/// A monitor that discards frames (used when monitoring is disabled or in tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullMonitor;

impl MonitorPort for NullMonitor {
    fn on_frames(&self, _frames: &[StereoFrame]) {}
}

/// The host's own speaker/headphone output for monitoring one mix.
pub struct LocalMonitor {
    _stream: cpal::Stream,
    producer: Mutex<rtrb::Producer<[f32; MONITOR_CHUNK_SAMPLES]>>,
    dropped_chunks: AtomicU64,
    sample_rate_hz: u32,
    channels: u16,
    device_name: String,
}

impl LocalMonitor {
    /// Start monitoring on `device_id` (or the default output when `None`).
    ///
    /// The device's own default output configuration is used as-is. The monitor does not resample
    /// or reconfigure anything: capture stays input-only, and the DSP already runs at the capture
    /// rate, so any needed conversion is the output device's own concern.
    pub fn start(device_id: Option<&str>) -> Result<Self, MonitorFault> {
        let host = cpal::default_host();
        let device = match device_id {
            Some(id) => find_output_device(&host, id)?,
            None => host.default_output_device().ok_or(MonitorFault::NoOutputDevice)?,
        };
        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "output".to_string());

        let config = device
            .default_output_config()
            .map_err(|_| MonitorFault::UnsupportedConfig)?;
        let channels = config.channels();
        let rate = config.sample_rate();

        let (producer, consumer) = rtrb::RingBuffer::<[f32; MONITOR_CHUNK_SAMPLES]>::new(
            MONITOR_QUEUE_CHUNKS,
        );
        let mut consumer = consumer;
        let mut scratch = [0.0f32; MONITOR_CHUNK_SAMPLES];
        let mut scratch_pos = 0usize;

        let stream = device
            .build_output_stream::<f32, _, _>(
                config.into(),
                move |out: &mut [f32], _| {
                    let mut written = 0usize;
                    while written < out.len() {
                        if scratch_pos == 0 {
                            match consumer.pop() {
                                Ok(chunk) => scratch = chunk,
                                Err(_) => {
                                    // Underrun: emit silence rather than repeating stale audio.
                                    for sample in &mut out[written..] {
                                        *sample = 0.0;
                                    }
                                    return;
                                }
                            }
                        }
                        let mono = channels == 1;
                        // Copy stereo frames into the device's channel layout (mono downmixes).
                        while written < out.len() && scratch_pos < MONITOR_CHUNK_SAMPLES {
                            if mono {
                                let l = scratch[scratch_pos];
                                let r = scratch[scratch_pos + 1];
                                out[written] = (l + r) * 0.5;
                                written += 1;
                                scratch_pos += 2;
                            } else {
                                out[written] = scratch[scratch_pos];
                                written += 1;
                                scratch_pos += 1;
                            }
                        }
                        if scratch_pos >= MONITOR_CHUNK_SAMPLES {
                            scratch_pos = 0;
                        }
                    }
                },
                |_err| {},
                None,
            )
            .map_err(|_| MonitorFault::StreamError)?;
        stream.play().map_err(|_| MonitorFault::StreamError)?;

        Ok(Self {
            _stream: stream,
            producer: Mutex::new(producer),
            dropped_chunks: AtomicU64::new(0),
            sample_rate_hz: rate,
            channels,
            device_name,
        })
    }

    /// The output rate the monitor is actually running at.
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// The output channel count.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The output device name, for operator display.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Frames dropped because the output could not keep up.
    pub fn dropped_chunks(&self) -> u64 {
        self.dropped_chunks.load(Ordering::Relaxed)
    }
}

impl MonitorPort for LocalMonitor {
    fn on_frames(&self, frames: &[StereoFrame]) {
        let Ok(mut producer) = self.producer.lock() else {
            return;
        };
        for frame in frames {
            if frame.frame_count == 0 {
                continue;
            }
            let mut chunk = [0.0f32; MONITOR_CHUNK_SAMPLES];
            let count = (frame.frame_count as usize).min(CODEC_FRAME_FRAMES as usize);
            chunk[..count * 2].copy_from_slice(&frame.pcm[..count * 2]);
            if producer.push(chunk).is_err() {
                // The output is behind; drop this chunk instead of blocking the DSP worker.
                self.dropped_chunks.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn find_output_device(host: &cpal::Host, device_id: &str) -> Result<cpal::Device, MonitorFault> {
    let devices = host.output_devices().map_err(|_| MonitorFault::NoOutputDevice)?;
    for device in devices {
        if let Ok(id) = device.id() {
            if id.to_string() == device_id {
                return Ok(device);
            }
        }
    }
    Err(MonitorFault::NoOutputDevice)
}

/// Enumerate output devices as `(device_id, name, is_default)`.
pub fn enumerate_output_devices() -> Vec<(String, String, bool)> {
    let host = cpal::default_host();
    let default_id = host.default_output_device().and_then(|d| d.id().ok());
    let mut out = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let Ok(id) = device.id() else { continue };
            let name = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| id.to_string());
            let is_default = default_id.as_ref() == Some(&id);
            out.push((id.to_string(), name, is_default));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::SessionContext;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn frame(value: f32) -> StereoFrame {
        let mut f = StereoFrame::zeroed(SessionContext::NONE);
        f.frame_count = CODEC_FRAME_FRAMES;
        for (i, sample) in f.pcm.iter_mut().enumerate() {
            *sample = value + i as f32 * 0.0;
        }
        f
    }

    /// A monitor that counts frames, proving the DSP→monitor hand-off without hardware.
    #[derive(Default)]
    struct CountingMonitor {
        frames: AtomicUsize,
    }
    impl MonitorPort for CountingMonitor {
        fn on_frames(&self, frames: &[StereoFrame]) {
            self.frames.fetch_add(frames.len(), Ordering::SeqCst);
        }
    }

    #[test]
    fn monitor_port_receives_produced_frames() {
        let monitor = Arc::new(CountingMonitor::default());
        let frames = [frame(1.0), frame(2.0)];
        monitor.on_frames(&frames);
        assert_eq!(monitor.frames.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn null_monitor_accepts_frames_without_panicking() {
        let frames = [frame(0.5)];
        NullMonitor.on_frames(&frames);
    }

    #[test]
    fn unknown_output_device_is_rejected_not_silently_defaulted() {
        let host = cpal::default_host();
        let result = find_output_device(&host, "definitely-not-a-device");
        assert!(matches!(result, Err(MonitorFault::NoOutputDevice)));
    }
}
