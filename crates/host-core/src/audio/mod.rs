//! DSP / mix engine boundary (Lane A, Task 3).
//!
//! Chain: `Σ sources → fixed headroom → master attenuation → stereo-linked probe limiter →
//! encoder`. The engine emits fixed [`CODEC_FRAME_FRAMES`]-frame stereo frames and carries a
//! partial frame across calls; partial state is discarded (never stale-replayed) when the
//! `audioEpoch`/`safetyGeneration` identity changes.

pub mod engine;
pub mod gain;
pub mod limiter;
pub mod safety;

#[cfg(test)]
mod tests;

/// Fixed bus headroom applied after summing sources.
///
/// Renamed from the earlier `WARNING_RANGES`. This is a **probe-only**, clearly-low-volume value
/// that requires real-hardware validation; it is not a hearing-safety guarantee.
pub const BUS_HEADROOM_DB: f32 = -27.0;

/// Frames per codec frame (initial 2.5 ms at 48 kHz).
pub const CODEC_FRAME_FRAMES: u32 = 120;

/// Maximum number of 120-frame frames one 256-frame input block can produce.
pub const MAX_OUT_FRAMES_PER_BLOCK: usize = 3;

pub use engine::AudioEngine;
pub use gain::{centered_coefficients, gain_linear, validate_gain};
pub use limiter::Limiter;
pub use safety::SafetyWorker;
