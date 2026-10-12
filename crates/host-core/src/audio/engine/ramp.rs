//! Fixed-step gain ramps (5 ms = 240 frames at 48 kHz) and the pan/balance position ramp.

/// Largest gain change per frame so a full −60..0 dB swing takes 240 frames.
pub const RAMP_DB_PER_FRAME: f32 = 60.0 / 240.0;

/// Frames a full `-1 -> +1` pan/balance travel takes at 48 kHz (5 ms).
pub const PAN_RAMP_FRAMES_PER_FULL_TRAVEL: f32 = 240.0;

/// Largest pan/balance change per frame so the full `2.0` span takes exactly
/// [`PAN_RAMP_FRAMES_PER_FULL_TRAVEL`] frames.
pub const PAN_STEP_PER_FRAME: f32 = 2.0 / PAN_RAMP_FRAMES_PER_FULL_TRAVEL;

/// A linear-in-dB ramp toward a target.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    current_db: f32,
    target_db: f32,
}

impl Ramp {
    /// A ramp initialized exactly at `db`.
    pub fn new(db: f32) -> Self {
        Self {
            current_db: db,
            target_db: db,
        }
    }

    /// Set the destination without jumping.
    pub fn set_target(&mut self, db: f32) {
        self.target_db = db;
    }

    /// Jump directly to the target (used when identity changes reset state).
    pub fn snap(&mut self) {
        self.current_db = self.target_db;
    }

    /// Current ramp value in dB.
    pub fn current_db(&self) -> f32 {
        self.current_db
    }

    /// Advance one frame and return the current value.
    pub fn next_db(&mut self) -> f32 {
        let delta = self.target_db - self.current_db;
        if delta.abs() <= RAMP_DB_PER_FRAME {
            self.current_db = self.target_db;
        } else {
            self.current_db += RAMP_DB_PER_FRAME * delta.signum();
        }
        self.current_db
    }
}

/// A linear position ramp toward a target over the pan/balance range `[-1.0, 1.0]`.
///
/// Unlike the dB [`Ramp`], this steps by a fixed absolute amount ([`PAN_STEP_PER_FRAME`]) so a full
/// `-1 -> +1` travel takes exactly [`PAN_RAMP_FRAMES_PER_FULL_TRAVEL`] frames independent of the
/// starting position.
#[derive(Debug, Clone, Copy)]
pub struct PositionRamp {
    current: f32,
    target: f32,
}

impl PositionRamp {
    /// A ramp initialized exactly at `position`.
    pub fn new(position: f32) -> Self {
        Self {
            current: position,
            target: position,
        }
    }

    /// Set the destination without jumping.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump directly to the target (used when identity changes reset state).
    pub fn snap(&mut self) {
        self.current = self.target;
    }

    /// The current position.
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Advance one frame and return the current position.
    ///
    /// The `next` name is fixed by the Task 2 interface even though it does not implement
    /// [`Iterator`].
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let delta = self.target - self.current;
        if delta.abs() <= PAN_STEP_PER_FRAME {
            self.current = self.target;
        } else {
            let stepped = self.current + PAN_STEP_PER_FRAME * delta.signum();
            // Snap when this step lands within one step of the target so the full-travel frame
            // count is exact despite f32 accumulation.
            self.current = if (self.target - stepped).abs() <= PAN_STEP_PER_FRAME {
                self.target
            } else {
                stepped
            };
        }
        self.current
    }
}
