//! Capture service lifecycle: SPSC pool wiring, telemetry, and the cpal backend.
//!
//! The capture data callback owns `free_rx` (slots to fill) and `ready_tx` (filled blocks)
//! privately on the capture thread. The DSP side consumes `ready_rx` and returns used slots via
//! `free_tx`. A slot is therefore always in exactly one of: the free queue, the ready queue, held
//! by the callback, or held by the DSP — which is what pool conservation checks.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use cpal::traits::{DeviceTrait, HostTrait};

use crate::capture::device::{SUPPORTED_RATES, is_supported_rate};
use crate::capture::timeline::{should_discard, Timeline};
use crate::capture::CAPTURE_SLOTS;
use crate::contract::{
    AudioSlot, CaptureBlock, CaptureFault, CaptureRequest, FaultCode, SampleFormat,
};
use crate::ids::{AudioEpoch, ChannelMapRevision, MAX_BLOCK_FRAMES, MAX_SOURCES};
use uuid::Uuid;

// ---------------------------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------------------------

const FAULT_NONE: u8 = 0;
const FAULT_DEVICE_UNAVAILABLE: u8 = 1;
const FAULT_UNSUPPORTED_RATE: u8 = 2;
const FAULT_UNSUPPORTED_FORMAT: u8 = 3;
const FAULT_CHANNEL_COUNT_TOO_LARGE: u8 = 4;
const FAULT_STREAM_ERROR: u8 = 5;
const FAULT_DISCONNECTED: u8 = 6;

fn encode_fault(fault: CaptureFault) -> u8 {
    match fault {
        CaptureFault::DeviceUnavailable => FAULT_DEVICE_UNAVAILABLE,
        CaptureFault::UnsupportedRate => FAULT_UNSUPPORTED_RATE,
        CaptureFault::UnsupportedFormat => FAULT_UNSUPPORTED_FORMAT,
        CaptureFault::ChannelCountTooLarge => FAULT_CHANNEL_COUNT_TOO_LARGE,
        CaptureFault::StreamError => FAULT_STREAM_ERROR,
        CaptureFault::Disconnected => FAULT_DISCONNECTED,
    }
}

fn decode_fault(raw: u8) -> FaultCode {
    let fault = match raw {
        FAULT_DEVICE_UNAVAILABLE => CaptureFault::DeviceUnavailable,
        FAULT_UNSUPPORTED_RATE => CaptureFault::UnsupportedRate,
        FAULT_UNSUPPORTED_FORMAT => CaptureFault::UnsupportedFormat,
        FAULT_CHANNEL_COUNT_TOO_LARGE => CaptureFault::ChannelCountTooLarge,
        FAULT_DISCONNECTED => CaptureFault::Disconnected,
        // `FAULT_NONE` and any unknown byte read as the unclassified capture fault.
        _ => CaptureFault::StreamError,
    };
    FaultCode::Capture(fault)
}

/// Lock-free capture-thread telemetry, shared with the DSP side via `Arc`.
#[derive(Debug)]
pub struct CaptureTelemetry {
    /// Latest captured source position (in frames).
    pub latest_position: AtomicU64,
    /// Frames dropped because no free slot was available or a block was stale.
    pub dropped_frames: AtomicU64,
    /// Blocks dropped for the same reasons.
    pub dropped_blocks: AtomicU64,
    /// Encoded [`CaptureFault`]; `FAULT_NONE` until a fault is recorded.
    pub fault: AtomicU8,
}

impl CaptureTelemetry {
    fn new() -> Self {
        Self {
            latest_position: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            dropped_blocks: AtomicU64::new(0),
            fault: AtomicU8::new(FAULT_NONE),
        }
    }

    /// The most recent capture fault.
    ///
    /// Until a fault is recorded this reports the unclassified `StreamError`, which is the
    /// contract's only neutral capture fault (the frozen [`FaultCode`] has no `None` variant).
    pub fn fault(&self) -> FaultCode {
        decode_fault(self.fault.load(Ordering::Acquire))
    }

    /// Record a capture fault (used by the backend error path and the fake test backend).
    pub(crate) fn set_fault(&self, fault: CaptureFault) {
        self.fault.store(encode_fault(fault), Ordering::Release);
    }

    fn record_drop(&self, frames: u64) {
        self.dropped_frames.fetch_add(frames, Ordering::Relaxed);
        self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------
// Backend seam
// ---------------------------------------------------------------------------------------------

/// Resolved input capability of a device for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureCapabilities {
    /// Full supported input channel count (never trimmed).
    pub channels: u16,
    /// Verified capture rate.
    pub sample_rate_hz: u32,
    /// Verified capture block size.
    pub buffer_frames: u32,
    /// Sample format the backend will deliver as `f32`.
    pub sample_format: SampleFormat,
}

/// The capture-thread resources handed to a backend when it builds a stream.
///
/// The backend owns the callback; tests inject a backend that never touches hardware.
pub struct CaptureSink {
    /// Capture generation carried by every block this stream publishes.
    pub audio_epoch: AudioEpoch,
    /// Interleaved-index-to-source map revision.
    pub channel_map_revision: ChannelMapRevision,
    /// Verified rate.
    pub sample_rate_hz: u32,
    /// Full supported input channel count.
    pub channel_count: u16,
    /// Slots the callback fills.
    pub free_rx: rtrb::Consumer<AudioSlot>,
    /// Filled blocks the callback publishes.
    pub ready_tx: rtrb::Producer<CaptureBlock>,
    /// Shared telemetry updated off the audio thread by the backend error path.
    pub telemetry: Arc<CaptureTelemetry>,
}

/// A running capture stream.
pub trait CaptureStream: Send {
    /// Start delivering callbacks.
    fn play(&mut self) -> Result<(), CaptureFault>;
}

/// Platform seam so tests never open real hardware.
pub trait CaptureBackend: Send + Sync {
    /// Resolve and validate the actual device capability for `req`.
    fn capabilities(&self, req: &CaptureRequest) -> Result<CaptureCapabilities, CaptureFault>;

    /// Build the stream, consuming the callback-side queue halves.
    fn build(
        &self,
        req: &CaptureRequest,
        caps: &CaptureCapabilities,
        sink: CaptureSink,
    ) -> Result<Box<dyn CaptureStream>, CaptureFault>;
}

// ---------------------------------------------------------------------------------------------
// Handle / lifecycle
// ---------------------------------------------------------------------------------------------

/// Opaque stop control; dropping the stream ends capture.
pub struct StopHandle {
    stream: Arc<Mutex<Option<Box<dyn CaptureStream>>>>,
    stopped: Arc<AtomicBool>,
}

impl StopHandle {
    /// Stop capture and release the device. Idempotent.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(mut guard) = self.stream.lock() {
            guard.take();
        }
    }

    /// Whether [`StopHandle::stop`] has been called.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for StopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StopHandle")
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

/// Lifecycle owner for a started capture; stop it through the returned handle.
#[derive(Debug)]
pub struct CaptureService {
    stop: StopHandle,
}

impl CaptureService {
    /// Stop capture.
    pub fn stop(&self) {
        self.stop.stop();
    }

    /// Whether capture has been stopped.
    pub fn is_stopped(&self) -> bool {
        self.stop.is_stopped()
    }
}

/// The DSP-side view of a running capture stream.
pub struct CaptureHandle {
    /// Filled blocks ready for the DSP.
    pub ready_rx: rtrb::Consumer<CaptureBlock>,
    /// Used slots returned by the DSP to the callback.
    pub free_tx: rtrb::Producer<AudioSlot>,
    /// Shared capture-thread telemetry.
    pub telemetry: Arc<CaptureTelemetry>,
    /// Full supported input channel count.
    pub configured_channels: u16,
    /// Verified capture rate.
    pub sample_rate_hz: u32,
    /// Stop control.
    pub stop: StopHandle,
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHandle")
            .field("configured_channels", &self.configured_channels)
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("telemetry", &self.telemetry)
            .finish()
    }
}

/// Mint a fresh opaque audio epoch, distinct from `_previous`.
///
/// A restart, remap, or rate change always mints a new epoch and resets partial assembly with no
/// backlog.
pub fn next_audio_epoch(_previous: AudioEpoch) -> AudioEpoch {
    AudioEpoch(Uuid::new_v4())
}

/// Start capture on the system CoreAudio backend.
pub fn start(
    req: CaptureRequest,
    audio_epoch: AudioEpoch,
    channel_map_revision: ChannelMapRevision,
) -> Result<CaptureHandle, CaptureFault> {
    start_with_backend(&SystemCaptureBackend, req, audio_epoch, channel_map_revision)
}

/// Start capture against an injected backend (tests never open real hardware).
pub fn start_with_backend<B: CaptureBackend + ?Sized>(
    backend: &B,
    req: CaptureRequest,
    audio_epoch: AudioEpoch,
    channel_map_revision: ChannelMapRevision,
) -> Result<CaptureHandle, CaptureFault> {
    if !is_supported_rate(req.sample_rate_hz) {
        return Err(CaptureFault::UnsupportedRate);
    }
    let caps = backend.capabilities(&req)?;

    // Channel counts above the native maximum are rejected, never trimmed.
    if caps.channels as usize > MAX_SOURCES {
        return Err(CaptureFault::ChannelCountTooLarge);
    }

    let telemetry = Arc::new(CaptureTelemetry::new());
    let (ready_tx, ready_rx) = rtrb::RingBuffer::<CaptureBlock>::new(CAPTURE_SLOTS);
    let (free_tx, free_rx) = rtrb::RingBuffer::<AudioSlot>::new(CAPTURE_SLOTS);

    let sink = CaptureSink {
        audio_epoch,
        channel_map_revision,
        sample_rate_hz: caps.sample_rate_hz,
        channel_count: caps.channels,
        free_rx,
        ready_tx,
        telemetry: Arc::clone(&telemetry),
    };

    let stream = backend.build(&req, &caps, sink)?;
    let stream = Arc::new(Mutex::new(Some(stream)));

    Ok(CaptureHandle {
        ready_rx,
        free_tx,
        telemetry,
        configured_channels: caps.channels,
        sample_rate_hz: caps.sample_rate_hz,
        stop: StopHandle {
            stream,
            stopped: Arc::new(AtomicBool::new(false)),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// System backend (cpal 0.18.2)
// ---------------------------------------------------------------------------------------------

/// The real CoreAudio backend. Only exercised by the ignored hardware test.
pub struct SystemCaptureBackend;

impl CaptureBackend for SystemCaptureBackend {
    fn capabilities(&self, req: &CaptureRequest) -> Result<CaptureCapabilities, CaptureFault> {
        let device = find_device(&req.device_id)?;
        let mut configs = device
            .supported_input_configs()
            .map_err(|_| CaptureFault::DeviceUnavailable)?;

        let mut best: Option<(u16, u32, SampleFormat)> = None;
        for range in configs.by_ref() {
            let channels = range.channels();
            let format = match range.sample_format() {
                cpal::SampleFormat::F32 => SampleFormat::F32,
                cpal::SampleFormat::I16 => SampleFormat::I16,
                cpal::SampleFormat::U16 => SampleFormat::U16,
                _ => continue,
            };
            // Confirmed cpal call: no `SampleRate` tuple constructor.
            if range.try_with_sample_rate(req.sample_rate_hz).is_some() {
                let replace = match best {
                    None => true,
                    Some((c, _, _)) => channels > c,
                };
                if replace {
                    best = Some((channels, req.sample_rate_hz, format));
                }
            }
        }

        let (channels, sample_rate_hz, sample_format) = best.ok_or_else(|| {
            if !SUPPORTED_RATES.contains(&req.sample_rate_hz) {
                CaptureFault::UnsupportedRate
            } else {
                CaptureFault::UnsupportedFormat
            }
        })?;

        // Prefer 64/128 when the device reports a supported range; otherwise defer to the device.
        let buffer_frames = match device.default_input_config() {
            Ok(default) => preferred_buffer(&default, req.buffer_frames),
            Err(_) => req.buffer_frames,
        };

        Ok(CaptureCapabilities {
            channels,
            sample_rate_hz,
            buffer_frames,
            sample_format,
        })
    }

    fn build(
        &self,
        req: &CaptureRequest,
        caps: &CaptureCapabilities,
        sink: CaptureSink,
    ) -> Result<Box<dyn CaptureStream>, CaptureFault> {
        let device = find_device(&req.device_id)?;
        let config = cpal::StreamConfig {
            channels: caps.channels,
            sample_rate: caps.sample_rate_hz,
            buffer_size: cpal::BufferSize::Fixed(caps.buffer_frames),
        };

        let CaptureSink {
            audio_epoch,
            channel_map_revision,
            sample_rate_hz,
            channel_count,
            mut free_rx,
            mut ready_tx,
            telemetry,
        } = sink;

        let mut timeline = Timeline::new();
        let err_telemetry = Arc::clone(&telemetry);
        let data_telemetry = Arc::clone(&telemetry);

        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    publish_chunks(
                        data,
                        channel_count as usize,
                        audio_epoch,
                        channel_map_revision,
                        sample_rate_hz,
                        channel_count,
                        &mut timeline,
                        &mut free_rx,
                        &mut ready_tx,
                        &data_telemetry,
                    );
                },
                move |_err| {
                    // Device loss: record the fault and stop publishing. Never panic on the RT thread.
                    err_telemetry.set_fault(CaptureFault::Disconnected);
                },
                None,
            )
            .map_err(|_| CaptureFault::StreamError)?;

        let stream: Box<dyn CaptureStream> = Box::new(SystemStream { inner: stream });
        Ok(stream)
    }
}

struct SystemStream {
    inner: cpal::Stream,
}

impl CaptureStream for SystemStream {
    fn play(&mut self) -> Result<(), CaptureFault> {
        use cpal::traits::StreamTrait;
        self.inner.play().map_err(|_| CaptureFault::StreamError)
    }
}

fn preferred_buffer(default: &cpal::SupportedStreamConfig, requested: u32) -> u32 {
    if let cpal::SupportedBufferSize::Range { min, max } = default.buffer_size() {
        for candidate in [64u32, 128u32] {
            if (*min..=*max).contains(&candidate) {
                return candidate;
            }
        }
        return requested.clamp(*min, *max);
    }
    requested
}

fn find_device(device_id: &str) -> Result<cpal::Device, CaptureFault> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|_| CaptureFault::DeviceUnavailable)?;
    for device in devices {
        if let Ok(id) = device.id() {
            if id.to_string() == device_id {
                return Ok(device);
            }
        }
    }
    Err(CaptureFault::DeviceUnavailable)
}

// ---------------------------------------------------------------------------------------------
// Shared callback publishing (used by the fake backend too)
// ---------------------------------------------------------------------------------------------

/// Copy up to [`MAX_BLOCK_FRAMES`] frames per published block, splitting wider callbacks.
///
/// No allocation, logging, locking, or I/O occurs here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_chunks(
    data: &[f32],
    input_channels: usize,
    audio_epoch: AudioEpoch,
    channel_map_revision: ChannelMapRevision,
    sample_rate_hz: u32,
    channel_count: u16,
    timeline: &mut Timeline,
    free_rx: &mut rtrb::Consumer<AudioSlot>,
    ready_tx: &mut rtrb::Producer<CaptureBlock>,
    telemetry: &CaptureTelemetry,
) {
    if input_channels == 0 {
        return;
    }
    let total_frames = data.len() / input_channels;
    let channels = input_channels.min(MAX_SOURCES);
    let mut offset = 0usize;

    while offset < total_frames {
        let remaining = total_frames - offset;
        let frames = remaining.min(MAX_BLOCK_FRAMES);
        let start = timeline.latest_position();
        let block_end = start + frames as u64;

        // 4 ms block-END freshness: a block entirely behind the window is dropped, not replayed.
        if should_discard(block_end, timeline.latest_position()) {
            timeline.on_drop(frames as u64);
            telemetry.record_drop(frames as u64);
            telemetry
                .latest_position
                .store(timeline.latest_position(), Ordering::Relaxed);
            offset += frames;
            continue;
        }

        let Some(mut slot) = free_rx.pop().ok() else {
            // No free slot: advance the timeline and count the dropped frames; never block.
            timeline.on_drop(frames as u64);
            telemetry.record_drop(frames as u64);
            telemetry
                .latest_position
                .store(timeline.latest_position(), Ordering::Relaxed);
            offset += frames;
            continue;
        };

        for frame in 0..frames {
            let src = (offset + frame) * input_channels;
            let dst = frame * channels;
            for ch in 0..channels {
                slot[dst + ch] = data[src + ch];
            }
        }

        timeline.on_block(frames as u64);
        telemetry
            .latest_position
            .store(timeline.latest_position(), Ordering::Relaxed);

        let block = CaptureBlock {
            audio_epoch,
            start_sample: start,
            frame_count: frames as u32,
            sample_rate_hz,
            channel_count,
            channel_map_revision,
            samples: slot,
        };
        if ready_tx.push(block).is_err() {
            // The ready queue is full and the slot comes back with the value.
            let _ = ready_tx;
            telemetry.record_drop(frames as u64);
        }
        offset += frames;
    }
}
