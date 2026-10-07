//! Capture → mix → encode pipeline.
//!
//! This is the composed audio path the POC was missing: one capture block flows through the
//! [`AudioEngine`] once per registered listener, and each listener's stereo output is encoded
//! into its own Opus packet. It is deliberately Sans-I/O so it can be exercised offline with a
//! synthetic capture block, and so the network layer can consume its output without owning any
//! audio state.
//!
//! Real-time discipline: after construction this performs **no heap allocation**. Outputs are
//! written into caller-owned storage.

use crate::audio::engine::MAX_SESSIONS;
use crate::audio::{AudioEngine, SafetyWorker, CODEC_FRAME_FRAMES, MAX_OUT_FRAMES_PER_BLOCK};
use crate::contract::{
    AudioFault, CaptureBlock, EncodedFrame, MixSnapshot, SessionContext, StereoFrame,
};
use crate::encoder::OpusWorker;
use crate::ids::SessionEpoch;

/// One listener's fixed-size output for a single capture block.
///
/// `count` frames were produced; only the first `count` entries of `frames` are valid.
pub struct ListenerOutput {
    /// Slot index into the pipeline's fixed listener table.
    pub slot: usize,
    /// The session this output belongs to.
    pub session: SessionEpoch,
    /// Number of valid encoded frames (0..=MAX_OUT_FRAMES_PER_BLOCK).
    pub count: usize,
    /// Encoded frames for this listener.
    pub frames: [EncodedFrame; MAX_OUT_FRAMES_PER_BLOCK],
}

impl ListenerOutput {
    /// An empty output for `slot`.
    pub fn empty(slot: usize) -> Self {
        Self {
            slot,
            session: SessionEpoch::from_bytes([0u8; 16]),
            count: 0,
            frames: [EncodedFrame::zeroed(); MAX_OUT_FRAMES_PER_BLOCK],
        }
    }
}

/// The composed capture → DSP → Opus pipeline.
///
/// A listener is registered by slot; `process` runs every registered slot's mix for one input
/// block. The encoder is unique per listener, so mixes can never share an encoded stream.
pub struct Pipeline {
    engine: AudioEngine,
    encoders: [Option<OpusWorker>; MAX_SESSIONS],
    sessions: [Option<SessionEpoch>; MAX_SESSIONS],
    safety: [Option<SafetyWorker>; MAX_SESSIONS],
    sample_rate_hz: u32,
}

impl Pipeline {
    /// Create a pipeline for a verified sample rate and resolved channel map.
    pub fn new(sample_rate_hz: u32, channel_map: &[crate::contract::ChannelMapEntry]) -> Self {
        Self {
            engine: AudioEngine::new(sample_rate_hz, channel_map),
            encoders: [None, None, None, None],
            sessions: [None, None, None, None],
            safety: [None, None, None, None],
            sample_rate_hz,
        }
    }

    /// The capture rate this pipeline was built for.
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// Register or replace one listener slot with its mix and session.
    pub fn register(
        &mut self,
        slot: usize,
        session: SessionEpoch,
        mix: MixSnapshot,
    ) -> Result<(), AudioFault> {
        if slot >= MAX_SESSIONS {
            return Err(AudioFault::Internal);
        }
        let encoder = OpusWorker::new().map_err(|_| AudioFault::Internal)?;
        let context = SessionContext {
            session_epoch: session,
            audio_epoch: mix.context.audio_epoch,
            safety_generation: mix.context.safety_generation,
        };
        self.engine.register_session(slot, context, mix);
        self.encoders[slot] = Some(encoder);
        self.sessions[slot] = Some(session);
        self.safety[slot] = Some(SafetyWorker::new(session));
        Ok(())
    }

    /// Remove a listener slot; its encoder and safety state are dropped.
    pub fn unregister(&mut self, slot: usize) {
        if slot < MAX_SESSIONS {
            self.encoders[slot] = None;
            self.sessions[slot] = None;
            self.safety[slot] = None;
        }
    }

    /// The session bound to a slot, if any.
    pub fn session_at(&self, slot: usize) -> Option<SessionEpoch> {
        self.sessions.get(slot).copied().flatten()
    }

    /// The safety worker for a slot, if registered (used by the control/arm path).
    pub fn safety_at(&self, slot: usize) -> Option<&SafetyWorker> {
        self.safety.get(slot).and_then(|s| s.as_ref())
    }

    /// Process one capture block for every registered listener.
    ///
    /// `snapshots` must be indexed by slot; a slot with no encoder, no session, or no snapshot
    /// produces nothing. Each output frame carries the listener's exact [`SessionContext`] so the
    /// transport safety gate can reject anything produced before a disarm.
    ///
    /// When `monitor` is set, that slot's produce-time **stereo PCM** (before encoding) is handed to
    /// the tap. Monitoring the pre-encode stereo is what the operator actually hears; it is also the
    /// last point where the mix is still a plain float buffer.
    pub fn process(
        &mut self,
        block: &CaptureBlock,
        snapshots: &[Option<MixSnapshot>; MAX_SESSIONS],
        out: &mut [ListenerOutput; MAX_SESSIONS],
        monitor: Option<(usize, &dyn crate::monitor::MonitorPort)>,
    ) -> Result<(), AudioFault> {
        for slot in 0..MAX_SESSIONS {
            let mut listener = ListenerOutput {
                slot,
                session: SessionEpoch::from_bytes([0u8; 16]),
                count: 0,
                frames: [EncodedFrame::zeroed(); MAX_OUT_FRAMES_PER_BLOCK],
            };
            let (Some(encoder), Some(session), Some(snapshot)) = (
                self.encoders[slot].as_mut(),
                self.sessions[slot],
                snapshots[slot].as_ref(),
            ) else {
                out[slot] = listener;
                continue;
            };
            listener.session = session;

            let mut stereo: [StereoFrame; MAX_OUT_FRAMES_PER_BLOCK] =
                [StereoFrame::zeroed(snapshot.context); MAX_OUT_FRAMES_PER_BLOCK];
            let produced = self.engine.process_block(slot, block, snapshot, &mut stereo)?;

            // Tap the monitored slot's stereo before it is encoded.
            if let Some((monitored, port)) = monitor {
                if monitored == slot && produced > 0 {
                    port.on_frames(&stereo[..produced]);
                }
            }

            for (index, frame) in stereo.iter().take(produced).enumerate() {
                let mut encoded = EncodedFrame::zeroed();
                encoder
                    .encode(frame, &mut encoded)
                    .map_err(|_| AudioFault::Internal)?;
                debug_assert_eq!(encoded.frame_count, CODEC_FRAME_FRAMES);
                listener.frames[index] = encoded;
            }
            listener.count = produced;
            out[slot] = listener;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ChannelMapEntry, MixSnapshot, SourceGain, SourceGainMatrix, SourceRole,
    };
    use crate::ids::{
        AudioEpoch, CatalogRevision, ChannelMapRevision, MixRevision, SafetyGeneration, SourceId,
        AUDIO_SLOT_SAMPLES,
    };

    fn source(index: u8) -> SourceId {
        SourceId::from_bytes([index; 16])
    }

    fn empty_outputs() -> [ListenerOutput; MAX_SESSIONS] {
        std::array::from_fn(ListenerOutput::empty)
    }

    fn channel_map() -> Vec<ChannelMapEntry> {
        vec![ChannelMapEntry {
            physical_index: 0,
            source_id: source(1),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        }]
    }

    fn snapshot(session: SessionEpoch, gain_db: f32) -> MixSnapshot {
        MixSnapshot {
            context: SessionContext {
                session_epoch: session,
                audio_epoch: AudioEpoch::from_bytes([0x55; 16]),
                safety_generation: SafetyGeneration(1),
            },
            catalog_revision: CatalogRevision(1),
            mix_revision: MixRevision(1),
            sources: SourceGainMatrix::from_slice(&[SourceGain {
                source_id: source(1),
                gain_db,
                muted: false,
            }]),
            master_db: 0.0,
            master_muted: false,
        }
    }

    fn block(value: f32, frames: u32) -> CaptureBlock {
        let mut samples = Box::new([0.0f32; AUDIO_SLOT_SAMPLES]);
        for (i, sample) in samples.iter_mut().enumerate() {
            *sample = value + (i % 7) as f32 * 0.01;
        }
        CaptureBlock {
            audio_epoch: AudioEpoch::from_bytes([0x55; 16]),
            start_sample: 0,
            frame_count: frames,
            sample_rate_hz: 48_000,
            channel_count: 1,
            channel_map_revision: ChannelMapRevision(1),
            samples,
        }
    }

    fn empty_snapshots() -> [Option<MixSnapshot>; MAX_SESSIONS] {
        [None, None, None, None]
    }

    #[test]
    fn one_block_produces_120_frame_encoded_frames_for_a_registered_listener() {
        let mut pipeline = Pipeline::new(48_000, &channel_map());
        let session = SessionEpoch::from_bytes([0x22; 16]);
        pipeline
            .register(0, session, snapshot(session, -6.0))
            .unwrap();

        let mut snapshots = empty_snapshots();
        snapshots[0] = Some(snapshot(session, -6.0));

        let mut out = empty_outputs();
        pipeline.process(&block(0.25, 256), &snapshots, &mut out, None).unwrap();

        assert_eq!(out[0].count, 2, "256 input frames yield two 120-frame packets");
        assert_eq!(out[0].session, session);
        assert_eq!(out[0].frames[0].context.session_epoch, session);
        assert!(
            out[0].frames[0].packet.len > 0,
            "encoded packet must be non-empty"
        );
        // Unregistered slots stay empty.
        assert_eq!(out[1].count, 0);
    }

    #[test]
    fn two_listeners_get_independent_encoded_streams() {
        let mut pipeline = Pipeline::new(48_000, &channel_map());
        let a = SessionEpoch::from_bytes([0x22; 16]);
        let b = SessionEpoch::from_bytes([0x23; 16]);
        pipeline.register(0, a, snapshot(a, -3.0)).unwrap();
        pipeline.register(1, b, snapshot(b, -12.0)).unwrap();

        let mut snapshots = empty_snapshots();
        snapshots[0] = Some(snapshot(a, -3.0));
        snapshots[1] = Some(snapshot(b, -12.0));

        let mut out = empty_outputs();
        pipeline.process(&block(0.5, 120), &snapshots, &mut out, None).unwrap();

        assert_eq!(out[0].count, 1);
        assert_eq!(out[1].count, 1);
        assert_eq!(out[0].frames[0].context.session_epoch, a);
        assert_eq!(out[1].frames[0].context.session_epoch, b);
        assert_ne!(
            out[0].frames[0].packet.bytes[..out[0].frames[0].packet.len as usize],
            out[1].frames[0].packet.bytes[..out[1].frames[0].packet.len as usize],
            "different personal mixes must not share an encoded stream"
        );
    }

    #[test]
    fn unregistered_listener_produces_nothing() {
        let mut pipeline = Pipeline::new(48_000, &channel_map());
        let snapshots = empty_snapshots();
        let mut out = empty_outputs();
        pipeline.process(&block(0.5, 120), &snapshots, &mut out, None).unwrap();
        assert!(out.iter().all(|o| o.count == 0));
    }

    #[test]
    fn unregister_stops_output_for_that_slot() {
        let mut pipeline = Pipeline::new(48_000, &channel_map());
        let session = SessionEpoch::from_bytes([0x22; 16]);
        pipeline.register(0, session, snapshot(session, -6.0)).unwrap();
        pipeline.unregister(0);

        let mut snapshots = empty_snapshots();
        snapshots[0] = Some(snapshot(session, -6.0));
        let mut out = empty_outputs();
        pipeline.process(&block(0.5, 120), &snapshots, &mut out, None).unwrap();
        assert_eq!(out[0].count, 0);
        assert!(pipeline.session_at(0).is_none());
    }
}
