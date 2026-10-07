//! RTP timestamp stepping for codec frame modes.

/// Codec frame duration modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RtpFrameMode {
    /// Initial 2.5 ms mode (120 frames at 48 kHz).
    Ms2p5,
    /// Later 5 ms probe (240 frames at 48 kHz).
    Ms5,
}

/// RTP timestamp step for a mode.
pub fn rtp_step_for(mode: RtpFrameMode) -> u32 {
    match mode {
        RtpFrameMode::Ms2p5 => 120,
        RtpFrameMode::Ms5 => 240,
    }
}
