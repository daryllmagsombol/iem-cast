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
/// Fixed POC policy: a single gain applied to the summed mix, independent of the number of active
/// sources (no adaptive/source-count normalization or AGC). At `-6.0 dB` a lone full-scale source
/// peaks at `-6 dBFS`, matching the stereo-linked limiter ceiling; correlated sums above that are
/// bounded by the limiter rather than by this constant. This is not a calibrated hearing-safety
/// guarantee.
pub const BUS_HEADROOM_DB: f32 = -6.0;

/// Frames per codec frame (2.5 ms at 48 kHz).
///
/// 2.5 ms (120 frames) is the latency-first default: it keeps the per-frame fill latency minimal.
/// A 5 ms frame (240 frames) would add ~2.5 ms of fill latency, which fights the low-latency goal,
/// so it is intentionally not chosen here.
pub const CODEC_FRAME_FRAMES: u32 = 120;

/// Maximum number of 120-frame frames one 256-frame input block can produce.
pub const MAX_OUT_FRAMES_PER_BLOCK: usize = 3;

pub use engine::AudioEngine;
pub use gain::{centered_coefficients, gain_linear, validate_gain};
pub use limiter::Limiter;
pub use safety::SafetyWorker;
