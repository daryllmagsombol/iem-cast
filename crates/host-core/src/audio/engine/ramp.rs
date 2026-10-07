//! Fixed-step gain ramps (5 ms = 240 frames at 48 kHz).

/// Largest gain change per frame so a full −60..0 dB swing takes 240 frames.
pub const RAMP_DB_PER_FRAME: f32 = 60.0 / 240.0;

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
