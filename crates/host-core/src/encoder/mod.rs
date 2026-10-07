//! Opus encoder boundary (Lane A, Task 3).

pub mod opus_worker;
pub mod rtp;

#[cfg(test)]
mod tests;

pub use opus_worker::OpusWorker;
pub use rtp::{rtp_step_for, RtpFrameMode};
