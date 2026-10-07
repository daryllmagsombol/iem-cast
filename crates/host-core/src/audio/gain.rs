//! Gain law and validation.
//!
//! Gains are −60..0 dB in 1 dB steps with no positive boost. Non-finite values are **rejected**,
//! never clamped to success; only finite out-of-range values are clamped to the bounds and
//! canonicalized.

use crate::contract::AudioFault;

/// Convert a decibel gain to a linear multiplier (`10^(db/20)`).
pub fn gain_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Validate a gain in dB.
///
/// Rejects non-finite values with [`AudioFault::InvalidSample`]. Finite values outside
/// `[-60, 0]` are clamped and canonicalized (e.g. `-70.0` -> `-60.0`, `6.0` -> `0.0`).
pub fn validate_gain(db: f32) -> Result<f32, AudioFault> {
    if !db.is_finite() {
        return Err(AudioFault::InvalidSample);
    }
    Ok(db.clamp(-60.0, 0.0))
}

/// Fixed unity coefficients that center a mono source into both stereo sides.
pub fn centered_coefficients() -> [f32; 2] {
    [1.0, 1.0]
}
