//! Stereo-linked probe limiter.
//!
//! Zero-lookahead instantaneous attack with a proposed ~50 ms release. This is a **probe option
//! only**: it does not promise hearing transparency, and the 0 / 0.5 / 1 ms lookahead comparison
//! is a later, gated experiment. A single shared gain ratio bounds the digital peak at −6 dBFS so
//! both channels are attenuated together (no image shift).

use crate::audio::gain::gain_linear;

/// Stereo-linked limiter with a shared gain.
#[derive(Debug, Clone, Copy)]
pub struct Limiter {
    gain: f32,
}

impl Limiter {
    /// Probe ceiling: the shared gain bounds the digital peak at −6 dBFS.
    pub const CEILING_DB: f32 = -6.0;
    /// Proposed release time at 48 kHz (50 ms).
    pub const RELEASE_FRAMES: u32 = 50 * 48;

    /// A limiter at unity (no reduction).
    pub fn new() -> Self {
        Self { gain: 1.0 }
    }

    /// Current shared gain ratio (<= 1.0).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Reduce the current shared gain by `db` for the next frame (used by tests/probes).
    pub fn reduction_db(&self) -> f32 {
        20.0 * self.gain.log10()
    }

    /// Process one stereo frame, returning the bounded L/R pair.
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let ceiling = gain_linear(Self::CEILING_DB);
        let peak = left.abs().max(right.abs());

        // Instantaneous attack: clamp the shared gain so this frame's peak cannot exceed the
        // ceiling, and apply that gain to this frame before any release recovery.
        if peak > 0.0 {
            let required = ceiling / peak;
            if required < self.gain {
                self.gain = required;
            }
        }

        let out = (left * self.gain, right * self.gain);

        // Linear release back toward unity; ~50 ms to fully recover (affects the next frame).
        if self.gain < 1.0 {
            let step = 1.0 / Self::RELEASE_FRAMES as f32;
            self.gain = (self.gain + step).min(1.0);
        }

        out
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}
