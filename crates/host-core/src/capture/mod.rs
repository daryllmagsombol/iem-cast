//! CoreAudio capture pool, source timeline, and device enumeration (Lane A, Task 2).
//!
//! Capture is input-only. The data callback owns the free-slot consumer and ready-slot producer
//! privately on the capture thread; the DSP side consumes ready blocks and returns used slots to
//! the free queue. Unit tests use [`tests::FakeCaptureBackend`] and never open real hardware.

pub mod device;
pub mod pool;
pub mod service;
pub mod timeline;

#[cfg(test)]
mod tests;

/// Number of preallocated capture slots in flight between the callback and the DSP.
pub const CAPTURE_SLOTS: usize = 8;

pub use crate::ids::MAX_BLOCK_FRAMES;
pub use device::enumerate_input_devices;
pub use pool::CapturePool;
pub use service::{
    next_audio_epoch, start, CaptureHandle, CaptureService, CaptureTelemetry, StopHandle,
};
pub use timeline::{should_discard, Timeline};
