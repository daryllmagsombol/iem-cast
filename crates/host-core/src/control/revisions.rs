//! Revision bookkeeping, gain canonicalization, and control-plane rate limiting.
//!
//! The control actor owns the authoritative revision counter. This module holds the small,
//! testable pieces: finite-only gain canonicalization and a bounded token-bucket limiter.

use std::time::Instant;

use crate::contract::ControlError;

/// Lowest permitted per-source gain in dB.
pub const MIN_GAIN_DB: f32 = -60.0;
/// Highest permitted per-source gain in dB (no positive boost).
pub const MAX_GAIN_DB: f32 = 0.0;
/// Control patch rate limit for one listener.
pub const PATCH_RATE_PER_SEC: f64 = 30.0;
/// Bounded burst allowance for the token bucket.
pub const PATCH_BURST: f64 = 60.0;

/// Canonicalize a finite gain to `[-60, 0]`; reject non-finite values outright.
///
/// Malformed/non-finite values are a rejection, never a silent clamp-to-success.
pub fn canonicalize_gain(db: f32) -> Result<f32, ControlError> {
    if !db.is_finite() {
        return Err(ControlError::Internal);
    }
    Ok(db.clamp(MIN_GAIN_DB, MAX_GAIN_DB))
}

/// A bounded token-bucket rate limiter driven by an injected clock.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Option<Instant>,
}

impl RateLimiter {
    /// Create a limiter with the given sustained rate and burst capacity.
    pub fn new(rate_per_sec: f64, burst: f64) -> Self {
        Self {
            capacity: burst.max(1.0),
            tokens: burst.max(1.0),
            refill_per_sec: rate_per_sec.max(0.0),
            last: None,
        }
    }

    /// Attempt to take one token at `now`.
    pub fn try_take(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last).as_secs_f64();
            self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        }
        self.last = Some(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
