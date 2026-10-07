//! Host runtime: the composition that actually drives audio.
//!
//! [`HostRuntime`] owns the capture stream on one side and the DSP/encode pipeline on the other.
//! A dedicated **non-audio** worker thread drains captured blocks, runs each registered
//! listener's mix through [`Pipeline`], hands the encoded frames to an [`EncodedSink`], and
//! returns the capture slot to the callback.
//!
//! The audio callback itself never runs any of this; it only fills a preallocated slot and pushes
//! it onto the SPSC queue. The worker is a normal thread, so it may allocate in the sink (for
//! example to hand a packet to the network layer) without touching the realtime path.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::audio::engine::MAX_SESSIONS;
use crate::capture::service::{
    start, start_with_backend, CaptureBackend, CaptureHandle, CaptureTelemetry, StopHandle,
};
use crate::contract::{CaptureFault, CaptureRequest, ChannelMapEntry, MixSnapshot};
use crate::ids::{AudioEpoch, ChannelMapRevision, SessionEpoch};
use crate::pipeline::{ListenerOutput, Pipeline};

/// One listener's encoded frames, handed to the sink after each processed capture block.
pub use crate::pipeline::ListenerOutput as EncodedBlock;

/// Receives encoded frames produced by the runtime worker.
///
/// Called on the worker thread — **not** the audio callback — so implementations may allocate,
/// log, or enqueue to a socket. Delivery must be non-blocking and must not panic. A sink may be
/// shared (for example an `Arc<MediaHub>`), so it takes `&self` and owns its own synchronization.
pub trait EncodedSink: Send + Sync + 'static {
    /// Deliver one block's worth of outputs, indexed by listener slot.
    fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]);
}

/// A sink that discards frames (useful when only telemetry is wanted).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EncodedSink for NullSink {
    fn on_block(&self, _outputs: &[ListenerOutput; MAX_SESSIONS]) {}
}

impl<F> EncodedSink for F
where
    F: Fn(&[ListenerOutput; MAX_SESSIONS]) + Send + Sync + 'static,
{
    fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
        self(outputs);
    }
}

impl<T> EncodedSink for Arc<T>
where
    T: EncodedSink + ?Sized,
{
    fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
        (**self).on_block(outputs);
    }
}

/// Commands sent from the owning thread to the runtime worker.
pub(crate) enum RuntimeCommand {
    SetMix {
        slot: usize,
        session: SessionEpoch,
        mix: MixSnapshot,
    },
    UpdateMix {
        slot: usize,
        mix: MixSnapshot,
    },
    Clear {
        slot: usize,
    },
}

/// A cloneable handle for sending mix changes into the DSP worker.
///
/// The bridge from the control plane to the DSP holds one of these instead of the whole
/// [`HostRuntime`], so control changes can arrive from the server thread while the runtime stays
/// owned by the host composition.
#[derive(Clone)]
pub struct RuntimeHandle {
    commands: mpsc::Sender<RuntimeCommand>,
}

impl RuntimeHandle {
    /// Register (or replace) a listener slot's session and mix.
    pub fn set_mix(&self, slot: usize, session: SessionEpoch, mix: MixSnapshot) {
        let _ = self.commands.send(RuntimeCommand::SetMix {
            slot,
            session,
            mix,
        });
    }

    /// Update only the mix for an already-registered slot.
    pub fn update_mix(&self, slot: usize, mix: MixSnapshot) {
        let _ = self.commands.send(RuntimeCommand::UpdateMix { slot, mix });
    }

    /// Remove a listener slot.
    pub fn clear_listener(&self, slot: usize) {
        let _ = self.commands.send(RuntimeCommand::Clear { slot });
    }
}

/// A running host: capture plus its DSP/encode worker.
pub struct HostRuntime {
    stop: StopHandle,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    handle: RuntimeHandle,
    telemetry: Arc<CaptureTelemetry>,
    sample_rate_hz: u32,
    configured_channels: u16,
}

impl HostRuntime {
    /// Start capture and the DSP worker.
    ///
    /// `channel_map` resolves physical capture indices to stable source ids. Capture never opens
    /// any output path, so it cannot feed back into the mixer.
    pub fn start<B, S>(
        backend: &B,
        req: CaptureRequest,
        audio_epoch: AudioEpoch,
        channel_map_revision: ChannelMapRevision,
        channel_map: Vec<ChannelMapEntry>,
        sink: S,
    ) -> Result<Self, CaptureFault>
    where
        B: CaptureBackend + ?Sized,
        S: EncodedSink,
    {
        let handle = start_with_backend(backend, req, audio_epoch, channel_map_revision)?;
        Self::from_handle(handle, channel_map, sink)
    }

    /// Start capture on the real device backend.
    pub fn start_real<S>(
        req: CaptureRequest,
        audio_epoch: AudioEpoch,
        channel_map_revision: ChannelMapRevision,
        channel_map: Vec<ChannelMapEntry>,
        sink: S,
    ) -> Result<Self, CaptureFault>
    where
        S: EncodedSink,
    {
        let handle = start(req, audio_epoch, channel_map_revision)?;
        Self::from_handle(handle, channel_map, sink)
    }

    fn from_handle<S>(
        handle: CaptureHandle,
        channel_map: Vec<ChannelMapEntry>,
        sink: S,
    ) -> Result<Self, CaptureFault>
    where
        S: EncodedSink,
    {
        let CaptureHandle {
            ready_rx,
            free_tx,
            telemetry,
            configured_channels,
            sample_rate_hz,
            stop,
        } = handle;

        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let (commands_tx, commands_rx) = mpsc::channel::<RuntimeCommand>();
        let sink = Arc::new(sink);

        let worker = thread::Builder::new()
            .name("iem-dsp".to_string())
            .spawn(move || {
                let mut pipeline = Pipeline::new(sample_rate_hz, &channel_map);
                let mut snapshots: [Option<MixSnapshot>; MAX_SESSIONS] = [None, None, None, None];
                let mut ready_rx = ready_rx;
                let mut free_tx = free_tx;

                while !worker_stopped.load(Ordering::Acquire) {
                    // Apply any pending control changes before touching audio.
                    while let Ok(command) = commands_rx.try_recv() {
                        apply_command(&mut pipeline, &mut snapshots, command);
                    }

                    let mut processed = false;
                    while let Ok(block) = ready_rx.pop() {
                        processed = true;
                        let mut outputs = std::array::from_fn(ListenerOutput::empty);
                        if pipeline.process(&block, &snapshots, &mut outputs).is_ok() {
                            sink.on_block(&outputs);
                        }
                        // Return the slot to the callback. Dropping on a full queue would leak the
                        // slot, but the callback owns the consumer side so it never fills here.
                        let _ = free_tx.push(block.samples);
                    }

                    if !processed {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            })
            .map_err(|_| CaptureFault::StreamError)?;

        Ok(Self {
            stop,
            stopped,
            worker: Some(worker),
            handle: RuntimeHandle {
                commands: commands_tx,
            },
            telemetry,
            sample_rate_hz,
            configured_channels,
        })
    }

    /// A cloneable handle for driving mix changes from another thread (e.g. the control plane).
    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    /// Register (or replace) the mix and session for a listener slot.
    pub fn set_mix(&self, slot: usize, session: SessionEpoch, mix: MixSnapshot) {
        self.handle.set_mix(slot, session, mix);
    }

    /// Update only the mix for an already-registered slot (gain/mute changes).
    pub fn update_mix(&self, slot: usize, mix: MixSnapshot) {
        self.handle.update_mix(slot, mix);
    }

    /// Remove a listener slot.
    pub fn clear_listener(&self, slot: usize) {
        self.handle.clear_listener(slot);
    }

    /// The capture rate in use.
    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// The full supported input channel count.
    pub fn configured_channels(&self) -> u16 {
        self.configured_channels
    }

    /// Shared capture telemetry (dropped frames, faults).
    pub fn telemetry(&self) -> &Arc<CaptureTelemetry> {
        &self.telemetry
    }

    /// Stop capture and join the worker. Idempotent.
    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.stop.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn apply_command(
    pipeline: &mut Pipeline,
    snapshots: &mut [Option<MixSnapshot>; MAX_SESSIONS],
    command: RuntimeCommand,
) {
    match command {
        RuntimeCommand::SetMix {
            slot,
            session,
            mix,
        } => {
            if pipeline.register(slot, session, mix).is_ok() {
                snapshots[slot] = Some(mix);
            }
        }
        RuntimeCommand::UpdateMix { slot, mix } => {
            snapshots[slot] = Some(mix);
        }
        RuntimeCommand::Clear { slot } => {
            pipeline.unregister(slot);
            snapshots[slot] = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::service::{CaptureCapabilities, CaptureSink, CaptureStream};
    use crate::contract::{
        CaptureBlock, SampleFormat, SourceGain, SourceGainMatrix, SourceRole,
    };
    use crate::ids::{CatalogRevision, MixRevision, SafetyGeneration, SourceId, MAX_BLOCK_FRAMES};
    use std::sync::atomic::AtomicUsize;

    /// A backend whose stream pushes synthetic blocks after `play()` starts a pusher thread.
    struct PushingBackend {
        channels: u16,
        frames: u32,
        /// How many blocks the pusher will attempt before exiting.
        blocks: u32,
    }

    impl CaptureBackend for PushingBackend {
        fn capabilities(&self, _req: &CaptureRequest) -> Result<CaptureCapabilities, CaptureFault> {
            Ok(CaptureCapabilities {
                channels: self.channels,
                sample_rate_hz: 48_000,
                buffer_frames: self.frames,
                sample_format: SampleFormat::F32,
            })
        }

        fn build(
            &self,
            _req: &CaptureRequest,
            _caps: &CaptureCapabilities,
            sink: CaptureSink,
        ) -> Result<Box<dyn CaptureStream>, CaptureFault> {
            Ok(Box::new(PusherStream {
                sink: Some(sink),
                channels: self.channels,
                frames: self.frames.min(MAX_BLOCK_FRAMES as u32),
                blocks: self.blocks,
            }))
        }
    }

    /// Owns the capture sink and spawns a pusher thread that stands in for the device callback.
    struct PusherStream {
        sink: Option<CaptureSink>,
        channels: u16,
        frames: u32,
        blocks: u32,
    }

    impl CaptureStream for PusherStream {
        fn play(&mut self) -> Result<(), CaptureFault> {
            let Some(mut sink) = self.sink.take() else {
                return Ok(());
            };
            let channels = self.channels;
            let frames = self.frames;
            let blocks = self.blocks;
            std::thread::spawn(move || {
                for _ in 0..blocks {
                    if let Ok(mut slot) = sink.free_rx.pop() {
                        for (i, sample) in slot.iter_mut().enumerate() {
                            // Only channel 0 carries signal, so the mix has something to encode.
                            *sample = if i % channels as usize == 0 { 0.4 } else { 0.0 };
                        }
                        let block = CaptureBlock {
                            audio_epoch: sink.audio_epoch,
                            start_sample: 0,
                            frame_count: frames,
                            sample_rate_hz: sink.sample_rate_hz,
                            channel_count: channels,
                            channel_map_revision: sink.channel_map_revision,
                            samples: slot,
                        };
                        let _ = sink.ready_tx.push(block);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            Ok(())
        }
    }

    fn source() -> SourceId {
        SourceId::from_bytes([0x11; 16])
    }

    fn channel_map() -> Vec<ChannelMapEntry> {
        vec![ChannelMapEntry {
            physical_index: 0,
            source_id: source(),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        }]
    }

    fn snapshot() -> MixSnapshot {
        MixSnapshot {
            context: crate::contract::SessionContext {
                session_epoch: SessionEpoch::from_bytes([0x22; 16]),
                audio_epoch: AudioEpoch::from_bytes([0x55; 16]),
                safety_generation: SafetyGeneration(1),
            },
            catalog_revision: CatalogRevision(1),
            mix_revision: MixRevision(1),
            sources: SourceGainMatrix::from_slice(&[SourceGain {
                source_id: source(),
                gain_db: -6.0,
                muted: false,
            }]),
            master_db: 0.0,
            master_muted: false,
        }
    }

    fn req() -> CaptureRequest {
        CaptureRequest {
            device_id: "fake".to_string(),
            sample_rate_hz: 48_000,
            buffer_frames: 128,
        }
    }

    /// Counts delivered encoded frames.
    #[derive(Default)]
    struct RecordingSink {
        frames: AtomicUsize,
    }
    impl EncodedSink for RecordingSink {
        fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
            for out in outputs {
                self.frames.fetch_add(out.count, Ordering::SeqCst);
            }
        }
    }

    #[test]
    fn runtime_delivers_encoded_frames_through_the_sink() {
        let sink = Arc::new(RecordingSink::default());
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 400,
        };

        let mut runtime = HostRuntime::start(
            &backend,
            req(),
            AudioEpoch::from_bytes([0x55; 16]),
            ChannelMapRevision(1),
            channel_map(),
            Arc::clone(&sink),
        )
        .expect("runtime starts");

        runtime.set_mix(0, SessionEpoch::from_bytes([0x22; 16]), snapshot());

        let mut waited = 0;
        while sink.frames.load(Ordering::SeqCst) == 0 && waited < 200 {
            thread::sleep(Duration::from_millis(5));
            waited += 1;
        }

        assert!(
            sink.frames.load(Ordering::SeqCst) > 0,
            "worker must deliver at least one encoded frame"
        );
        runtime.stop();
    }

    #[test]
    fn stop_is_idempotent() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 1,
        };
        let mut runtime = HostRuntime::start(
            &backend,
            req(),
            AudioEpoch::from_bytes([0x55; 16]),
            ChannelMapRevision(1),
            channel_map(),
            NullSink,
        )
        .expect("runtime starts");
        runtime.stop();
        runtime.stop();
    }

    #[test]
    fn telemetry_is_exposed_after_start() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 1,
        };
        let runtime = HostRuntime::start(
            &backend,
            req(),
            AudioEpoch::from_bytes([0x55; 16]),
            ChannelMapRevision(1),
            channel_map(),
            NullSink,
        )
        .expect("runtime starts");
        assert_eq!(runtime.sample_rate_hz(), 48_000);
        assert_eq!(runtime.configured_channels(), 1);
        assert_eq!(runtime.telemetry().dropped_frames.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unsupported_rate_is_rejected() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 1,
        };
        let mut bad = req();
        bad.sample_rate_hz = 44_100;
        let result = HostRuntime::start(
            &backend,
            bad,
            AudioEpoch::from_bytes([0x55; 16]),
            ChannelMapRevision(1),
            channel_map(),
            NullSink,
        );
        assert!(matches!(result, Err(CaptureFault::UnsupportedRate)));
    }
}

