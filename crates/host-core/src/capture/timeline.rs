//! Source timeline accounting.
//!
//! The timeline advances on every captured block and on every drop so a downstream gap can never
//! be silently replayed. Freshness is measured against the block **end**; a block more than 4 ms
//! behind the latest captured position is stale and discarded without replay.

/// Frames in the 4 ms freshness window at the POC's 48 kHz rate.
pub const FRESHNESS_FRAMES: u64 = 4 * 48;

/// Monotonic source position with explicit drop accounting.
#[derive(Debug, Default)]
pub struct Timeline {
    latest: u64,
    dropped_frames: u64,
    dropped_blocks: u64,
}

impl Timeline {
    /// A timeline at position zero with no drops.
    pub fn new() -> Self {
        Self::default()
    }

    /// Latest captured sample position.
    pub fn latest_position(&self) -> u64 {
        self.latest
    }

    /// Total frames dropped since capture started.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Total blocks dropped since capture started.
    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks
    }

    /// Advance by a successfully published block.
    pub fn on_block(&mut self, frames: u64) {
        self.latest += frames;
    }

    /// Advance by a dropped chunk and record it. The gap is not replayed.
    pub fn on_drop(&mut self, frames: u64) {
        self.latest += frames;
        self.dropped_frames += frames;
        self.dropped_blocks += 1;
    }
}

/// Whether a ready block ending at `block_end` is more than 4 ms behind `latest`.
///
/// The comparison uses the block **end** (not its start) so a full-width block that has just
/// finished is fresh.
pub fn should_discard(block_end: u64, latest: u64) -> bool {
    latest.saturating_sub(block_end) > FRESHNESS_FRAMES
}
