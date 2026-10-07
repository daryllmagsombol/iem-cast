//! Deterministic capture-pool accounting.
//!
//! This models the fixed slot pool so pool conservation (including slots held in flight by the
//! callback and the DSP) can be asserted directly in unit tests. The production capture path uses
//! paired `rtrb` SPSC queues with the same slot count.

use std::collections::VecDeque;

use crate::capture::CAPTURE_SLOTS;
use crate::contract::AudioSlot;
use crate::contract::silent_audio_slot;

/// Fixed free/ready slot pool with explicit in-flight holds.
#[derive(Debug)]
pub struct CapturePool {
    free: VecDeque<AudioSlot>,
    ready: VecDeque<AudioSlot>,
    callback_held: usize,
    dsp_held: usize,
}

impl CapturePool {
    /// A pool with all ocho slots free and none in flight.
    pub fn new() -> Self {
        let mut free = VecDeque::with_capacity(CAPTURE_SLOTS);
        for _ in 0..CAPTURE_SLOTS {
            free.push_back(silent_audio_slot());
        }
        Self {
            free,
            ready: VecDeque::new(),
            callback_held: 0,
            dsp_held: 0,
        }
    }

    /// Take a free slot for the capture callback to fill.
    pub fn acquire_free(&mut self) -> Option<AudioSlot> {
        self.free.pop_front()
    }

    /// Publish a filled slot into the ready queue; returns the native frame count.
    pub fn publish(&mut self, slot: AudioSlot, frames: usize) -> usize {
        self.ready.push_back(slot);
        frames
    }

    /// Reserve the next free slot on behalf of the callback thread.
    pub fn callback_hold(&mut self) -> Option<AudioSlot> {
        let slot = self.free.pop_front()?;
        self.callback_held += 1;
        Some(slot)
    }

    /// Reserve a ready slot on behalf of the DSP thread.
    pub fn dsp_hold(&mut self) -> Option<AudioSlot> {
        let slot = self.ready.pop_front()?;
        self.dsp_held += 1;
        Some(slot)
    }

    /// Number of free slots not handed to either thread.
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    /// Number of published slots awaiting DSP consumption.
    pub fn ready_count(&self) -> usize {
        self.ready.len()
    }

    /// Slots reserved by the callback thread.
    pub fn callback_held(&self) -> usize {
        self.callback_held
    }

    /// Slots reserved by the DSP thread.
    pub fn dsp_held(&self) -> usize {
        self.dsp_held
    }
}

impl Default for CapturePool {
    fn default() -> Self {
        Self::new()
    }
}
