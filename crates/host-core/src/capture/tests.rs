//! Capture lane unit tests.
//!
//! Every test here uses [`FakeCaptureBackend`] or the deterministic [`CapturePool`] and never
//! opens real hardware. The only hardware test is `#[ignore]`d and requires `IEM_TEST_DEVICE_ID`.

use std::sync::Mutex;

use super::pool::CapturePool;
use super::service::{
    start_with_backend, CaptureBackend, CaptureCapabilities, CaptureSink, CaptureStream,
    CaptureTelemetry,
};
use super::timeline::{should_discard, Timeline};
use super::{enumerate_input_devices, next_audio_epoch};
use crate::contract::{CaptureFault, CaptureRequest, FaultCode, SampleFormat};
use crate::ids::{AudioEpoch, ChannelMapRevision};

// ---------------------------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------------------------

/// A backend that satisfies the capture seam without touching CoreAudio.
pub struct FakeCaptureBackend {
    channels: u16,
    sample_rate_hz: u32,
    buffer_frames: u32,
    telemetry: Mutex<Option<std::sync::Arc<CaptureTelemetry>>>,
}

impl FakeCaptureBackend {
    /// A 2-channel 48 kHz/128-frame fake device.
    pub fn new() -> Self {
        Self {
            channels: 2,
            sample_rate_hz: 48_000,
            buffer_frames: 128,
            telemetry: Mutex::new(None),
        }
    }

    /// Simulate the device disappearing after a successful start.
    pub fn inject_disconnect(&self) {
        if let Some(telemetry) = self.telemetry.lock().unwrap().as_ref() {
            telemetry.set_fault(CaptureFault::Disconnected);
        }
    }
}

impl Default for FakeCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct FakeStream;

impl CaptureStream for FakeStream {
    fn play(&mut self) -> Result<(), CaptureFault> {
        Ok(())
    }
}

impl CaptureBackend for FakeCaptureBackend {
    fn capabilities(&self, _req: &CaptureRequest) -> Result<CaptureCapabilities, CaptureFault> {
        Ok(CaptureCapabilities {
            channels: self.channels,
            sample_rate_hz: self.sample_rate_hz,
            buffer_frames: self.buffer_frames,
            sample_format: SampleFormat::F32,
        })
    }

    fn build(
        &self,
        _req: &CaptureRequest,
        _caps: &CaptureCapabilities,
        sink: CaptureSink,
    ) -> Result<Box<dyn CaptureStream>, CaptureFault> {
        *self.telemetry.lock().unwrap() = Some(std::sync::Arc::clone(&sink.telemetry));
        Ok(Box::new(FakeStream))
    }
}

fn capture_req_48k() -> CaptureRequest {
    CaptureRequest {
        device_id: "fake-device".to_string(),
        sample_rate_hz: 48_000,
        buffer_frames: 128,
    }
}

// ---------------------------------------------------------------------------------------------
// Pool and timeline
// ---------------------------------------------------------------------------------------------

#[test]
fn pool_conservation_counts_in_flight_slots() {
    let mut pool = CapturePool::new();
    let a = pool.acquire_free().expect("free slot");
    let _ = pool.publish(a, 128usize);
    let _c = pool.callback_hold();
    let _d = pool.dsp_hold();
    assert_eq!(
        pool.free_count() + pool.ready_count() + pool.callback_held() + pool.dsp_held(),
        8
    );
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
    assert!(should_discard(100, 100 + 4 * 48 + 1));
    assert!(!should_discard(100 + 4 * 48, 100 + 4 * 48));
}

#[test]
fn captured_chunk_is_the_real_callback_width_not_forced_256() {
    let mut pool = CapturePool::new();
    let s = pool.acquire_free().unwrap();
    let fc = pool.publish(s, 128usize);
    assert_eq!(fc, 128);
}

// ---------------------------------------------------------------------------------------------
// Service / device loss
// ---------------------------------------------------------------------------------------------

#[test]
fn device_loss_via_fake_backend_emits_fault_and_new_audio_epoch() {
    let backend = FakeCaptureBackend::new();
    let h = start_with_backend(
        &backend,
        capture_req_48k(),
        AudioEpoch::from_bytes([7; 16]),
        ChannelMapRevision(1),
    )
    .unwrap();
    backend.inject_disconnect();
    assert_eq!(
        h.telemetry.fault(),
        FaultCode::Capture(CaptureFault::Disconnected)
    );
    assert_ne!(
        next_audio_epoch(AudioEpoch::from_bytes([7; 16])),
        AudioEpoch::from_bytes([7; 16])
    );
}

// ---------------------------------------------------------------------------------------------
// Hardware-gated device test
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "requires a connected Soundcraft device; run manually only in Task 10"]
fn coreaudio_lists_the_selected_device_and_accepts_48000() {
    let id = std::env::var("IEM_TEST_DEVICE_ID")
        .expect("set IEM_TEST_DEVICE_ID to the Soundcraft device id");
    let d = enumerate_input_devices()
        .into_iter()
        .find(|d| d.device_id == id)
        .expect("selected device");
    assert_eq!(d.sample_rate_hz, Some(48_000));
}
