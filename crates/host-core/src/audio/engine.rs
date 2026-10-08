//! Fixed-step mix engine.
//!
//! `process_block` reads a multichannel [`CaptureBlock`] and emits up to
//! [`MAX_OUT_FRAMES_PER_BLOCK`] codec frames of [`CODEC_FRAME_FRAMES`] stereo frames. A partial
//! frame is carried across calls; when the session identity (`audioEpoch`/`safetyGeneration`)
//! changes the partial frame is discarded rather than stale-replayed.

pub mod ramp;

use crate::audio::gain::{centered_coefficients, gain_linear, validate_gain};
use crate::audio::limiter::Limiter;
use crate::audio::{BUS_HEADROOM_DB, CODEC_FRAME_FRAMES, MAX_OUT_FRAMES_PER_BLOCK};
use crate::contract::{
    AudioFault, CaptureBlock, ChannelMapEntry, MixSnapshot, SessionContext, SourceGain,
    SourceRole, StereoFrame,
};
use crate::ids::{SourceId, MAX_SOURCES, STEREO_PCM_SAMPLES};
use ramp::Ramp;

/// Mute is a boolean logical −∞, never numeric infinity; −120 dB is the near-silent floor.
const MUTE_FLOOR_DB: f32 = -120.0;

/// Fixed number of listener sessions the POC engine may hold.
pub const MAX_SESSIONS: usize = 4;

struct SessionState {
    context: SessionContext,
    partial: [f32; STEREO_PCM_SAMPLES],
    partial_frames: u32,
    partial_start: u64,
    ramps: [Ramp; MAX_SOURCES],
    master_ramp: Ramp,
    limiter: Limiter,
}

/// Per-session mix engine.
pub struct AudioEngine {
    #[allow(dead_code)]
    sample_rate_hz: u32,
    channel_map: Vec<ChannelMapEntry>,
    sessions: Vec<Option<SessionState>>,
    headroom_linear: f32,
}

impl AudioEngine {
    /// Create an engine for a verified sample rate and resolved channel map.
    pub fn new(sample_rate_hz: u32, channel_map: &[ChannelMapEntry]) -> Self {
        let channel_map: Vec<ChannelMapEntry> =
            channel_map.iter().take(MAX_SOURCES).copied().collect();
        let mut sessions = Vec::with_capacity(MAX_SESSIONS);
        for _ in 0..MAX_SESSIONS {
            sessions.push(None);
        }
        Self {
            sample_rate_hz,
            channel_map,
            sessions,
            headroom_linear: gain_linear(BUS_HEADROOM_DB),
        }
    }

    /// Register (or upsert) the mix for one listener slot.
    ///
    /// The [`SessionContext`] is the safety identity. When the incoming context is exactly equal to
    /// the slot's current context, the partial PCM, gain ramps, and limiter are **preserved** so an
    /// ordinary gain/mute/revision edit does not introduce a sample gap or a ramp discontinuity. A
    /// new identity (session, audio epoch, or safety generation) resets all state fresh, never
    /// re-tagging the old partial.
    pub fn register_session(&mut self, slot: usize, ctx: SessionContext, mix: MixSnapshot) {
        if slot >= self.sessions.len() {
            return;
        }
        // Same exact safety identity: preserve partial/ramps/limiter (and encoder/output state held
        // by the caller). Gain/mute targets are re-resolved on every `process_block`, so there is
        // nothing to rebuild here and smoothing continues from the current ramp positions.
        if let Some(state) = self.sessions[slot].as_ref() {
            if state.context == ctx {
                return;
            }
        }
        let mut ramps = [Ramp::new(0.0); MAX_SOURCES];
        for (index, entry) in self.channel_map.iter().enumerate() {
            let gain = find_gain(&mix, entry.source_id);
            let db = if gain.muted { MUTE_FLOOR_DB } else { gain.gain_db };
            ramps[index] = Ramp::new(db.clamp(-60.0, 0.0));
        }
        let mut master_ramp = Ramp::new(
            if mix.master_muted {
                MUTE_FLOOR_DB
            } else {
                mix.master_db
            }
            .clamp(-60.0, 0.0),
        );
        master_ramp.snap();
        self.sessions[slot] = Some(SessionState {
            context: ctx,
            partial: [0.0f32; STEREO_PCM_SAMPLES],
            partial_frames: 0,
            partial_start: 0,
            ramps,
            master_ramp,
            limiter: Limiter::new(),
        });
    }

    /// The exact safety context installed for `slot`, if any.
    ///
    /// Used by [`crate::pipeline::Pipeline`] to decide whether an upsert can preserve state without
    /// duplicating the identity table.
    pub(crate) fn context(&self, slot: usize) -> Option<SessionContext> {
        self.sessions
            .get(slot)
            .and_then(|s| s.as_ref())
            .map(|s| s.context)
    }

    /// Drop all engine state for `slot` (partial PCM, ramps, limiter, context).
    pub(crate) fn clear_session(&mut self, slot: usize) {
        if let Some(entry) = self.sessions.get_mut(slot) {
            *entry = None;
        }
    }

    /// Process one input block for `slot`, writing up to
    /// [`MAX_OUT_FRAMES_PER_BLOCK`] codec frames and returning how many were produced.
    pub fn process_block(
        &mut self,
        slot: usize,
        input: &CaptureBlock,
        snapshot: &MixSnapshot,
        out: &mut [StereoFrame; MAX_OUT_FRAMES_PER_BLOCK],
    ) -> Result<usize, AudioFault> {
        let headroom = self.headroom_linear;
        let channel_map = &self.channel_map;
        let state = self
            .sessions
            .get_mut(slot)
            .and_then(|s| s.as_mut())
            .ok_or(AudioFault::Internal)?;

        // The incoming capture block must belong to the snapshot's audio epoch. Validate it against
        // `snapshot.context.audio_epoch` BEFORE any context state change or appending: comparing
        // against the old `state.context.audio_epoch` would let an old-epoch block be retagged as
        // the new epoch, or reject a valid new-epoch block while the old state is still installed.
        if input.audio_epoch != snapshot.context.audio_epoch {
            return Ok(0);
        }

        // Identity change resets partial assembly; never stale-replay the held tail. Gain/mute
        // edits that keep the exact context do NOT reset the partial or ramps.
        let identity_changed = state.context != snapshot.context;
        if identity_changed {
            state.partial_frames = 0;
            state.context = snapshot.context;
        }

        // A timeline gap means the held partial is no longer contiguous with this block. Keep the
        // encoder (caller-owned) but drop the incomplete PCM and re-anchor to this block's real
        // start; do not synthesize silence.
        if state.partial_frames > 0 {
            let expected = state
                .partial_start
                .saturating_add(state.partial_frames as u64);
            if input.start_sample != expected {
                state.partial_frames = 0;
            }
        }

        // Resolve and validate targets before touching samples.
        for (index, entry) in channel_map.iter().enumerate() {
            let gain = find_gain(snapshot, entry.source_id);
            let db = validate_gain(gain.gain_db)?;
            let target_db = if gain.muted { MUTE_FLOOR_DB } else { db };
            state.ramps[index].set_target(target_db);
        }
        let master_db = validate_gain(snapshot.master_db)?;
        let master_target = if snapshot.master_muted {
            MUTE_FLOOR_DB
        } else {
            master_db
        };
        state.master_ramp.set_target(master_target);

        // A new identity snaps to the installed targets instead of ramping from the old epoch.
        if identity_changed {
            for ramp in state.ramps.iter_mut() {
                ramp.snap();
            }
            state.master_ramp.snap();
        }

        let channels = input.channel_count as usize;
        let mut produced = 0usize;

        for frame in 0..input.frame_count {
            if state.partial_frames == 0 {
                state.partial_start = input.start_sample + frame as u64;
            }

            let mut left = 0f32;
            let mut right = 0f32;

            for (index, entry) in channel_map.iter().enumerate() {
                let physical = entry.physical_index as usize;
                let gain = gain_linear(state.ramps[index].next_db());
                if physical >= channels {
                    continue;
                }
                let sample = input.samples[frame as usize * channels + physical];
                let scaled = sample * gain;
                match entry.role {
                    SourceRole::InputChannel => {
                        let [l, r] = centered_coefficients();
                        left += scaled * l;
                        right += scaled * r;
                    }
                    SourceRole::MasterLr => {
                        if entry.physical_index % 2 == 0 {
                            left += scaled;
                        } else {
                            right += scaled;
                        }
                    }
                }
            }

            left *= headroom;
            right *= headroom;

            let master_gain = gain_linear(state.master_ramp.next_db());
            let (limited_l, limited_r) =
                state.limiter.process(left * master_gain, right * master_gain);

            let idx = state.partial_frames as usize * 2;
            state.partial[idx] = limited_l;
            state.partial[idx + 1] = limited_r;
            state.partial_frames += 1;

            if state.partial_frames == CODEC_FRAME_FRAMES {
                let mut sf = StereoFrame::zeroed(snapshot.context);
                sf.start_sample = state.partial_start;
                sf.frame_count = CODEC_FRAME_FRAMES;
                sf.pcm.copy_from_slice(&state.partial);
                out[produced] = sf;
                produced += 1;
                state.partial_frames = 0;
                if produced == MAX_OUT_FRAMES_PER_BLOCK {
                    break;
                }
            }
        }

        Ok(produced)
    }
}

fn find_gain(snapshot: &MixSnapshot, source_id: SourceId) -> SourceGain {
    snapshot
        .sources
        .as_slice()
        .iter()
        .find(|g| g.source_id == source_id)
        .copied()
        .unwrap_or(SourceGain {
            source_id,
            gain_db: 0.0,
            muted: false,
        })
}
