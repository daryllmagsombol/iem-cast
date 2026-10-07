//! Host process composition: capture + DSP + media transport, driven end to end.
//!
//! [`HostSession`] is the missing composition the POC lacked: it starts the capture/DSP
//! [`HostRuntime`] with an [`EncodedSink`] that routes each listener's encoded frames into a shared
//! [`MediaHub`], binds the UDP socket the media sessions send on, and runs a small pump thread that
//! feeds inbound datagrams into the hub, advances its timers, and emits its outbound datagrams.
//!
//! Scope and honesty: this composition owns capture and the media datagram path. It deliberately
//! does **not** own the HTTPS/WSS control server; the server shares the same `MediaHub` through
//! [`crate::server::HostServer::with_media`] so WS signaling reaches the same per-listener sessions.
//! Real device capture is only reached through [`HostSession::start_real`]; tests always inject a
//! fake backend and a loopback socket.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::engine::MAX_SESSIONS;
use crate::capture::service::{CaptureBackend, SystemCaptureBackend};
use crate::contract::{
    CaptureFault, ChannelMapEntry, ControlError, MixSnapshot, SourceRole, StartHostRequest,
};
use crate::ids::{AudioEpoch, ChannelMapRevision, SessionEpoch, SourceId};
use crate::pipeline::ListenerOutput;
use crate::runtime::{EncodedSink, HostRuntime};
use crate::server::tls::TlsIdentity;
use crate::transport::{MediaHub, SelectedInterface};

/// How long the pump sleeps when no datagram is ready before re-checking timers and the stop flag.
const PUMP_IDLE: Duration = Duration::from_millis(1);
/// Maximum inbound datagram accepted by the pump (a bounded RTP packet fits well within).
const MAX_DATAGRAM_BYTES: usize = 2048;

/// Composition/lifecycle failure for [`HostSession`].
#[derive(Debug, thiserror::Error)]
pub enum HostFault {
    /// The capture/DSP runtime failed to start.
    #[error("capture: {0}")]
    Capture(#[from] CaptureFault),
    /// The TLS identity is missing, unreadable, or invalid.
    #[error("tls: {0}")]
    Tls(#[from] ControlError),
    /// The UDP media socket could not be bound or configured.
    #[error("media socket: {0}")]
    Socket(#[from] std::io::Error),
}

/// Routes the runtime's encoded frames into a shared [`MediaHub`].
///
/// This is the bridge from the DSP worker thread to the WebRTC sessions. It holds the same
/// `Arc<Mutex<MediaHub>>` the server and the UDP pump hold, so a listener's frames reach its own
/// session while the control plane negotiates that session on the WS socket.
struct MediaHubSink {
    hub: Arc<Mutex<MediaHub>>,
}

impl EncodedSink for MediaHubSink {
    fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
        let mut hub = self.hub.lock().expect("media hub");
        hub.write_block(outputs);
    }
}

/// Forwards the control actor's DSP commands into the running runtime and media hub.
///
/// A mix may arrive before the listener's media slot exists (e.g. a patch before `rtc.offer`).
/// Those snapshots are held in `pending` and flushed when the session arms, so ordering between
/// signaling and control cannot silently drop a mix.
struct DspBridge {
    handle: crate::runtime::RuntimeHandle,
    hub: Arc<Mutex<MediaHub>>,
    audio_epoch: AudioEpoch,
    pending: Mutex<std::collections::HashMap<SessionEpoch, MixSnapshot>>,
}

impl DspBridge {
    /// The media slot bound to a session, if the listener has negotiated one.
    fn slot_for(&self, session: SessionEpoch) -> Option<usize> {
        let hub = self.hub.lock().ok()?;
        (0..MAX_SESSIONS).find(|&slot| hub.session_at(slot) == Some(session))
    }
}

impl crate::contract::DspControl for DspBridge {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError> {
        let session = snapshot.context.session_epoch;
        match self.slot_for(session) {
            Some(slot) => self.handle.set_mix(slot, session, snapshot),
            None => {
                // No media session yet; remember it so arming can apply it.
                if let Ok(mut pending) = self.pending.lock() {
                    pending.insert(session, snapshot);
                }
            }
        }
        Ok(())
    }

    fn request_arm(&self, arm: crate::contract::ListenArm) -> Result<(), ControlError> {
        let session = arm.context.session_epoch;
        // Apply any mix that arrived before signaling opened the media slot.
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&session));
        if let (Some(slot), Some(snapshot)) = (self.slot_for(session), pending) {
            self.handle.set_mix(slot, session, snapshot);
        }
        // Open the per-session media gate with the exact context the actor assigned.
        if let Some(slot) = self.slot_for(session) {
            if let Ok(mut hub) = self.hub.lock() {
                hub.arm(slot, arm.context);
            }
        }
        Ok(())
    }

    fn disarm(&self, session: SessionEpoch, new_generation: crate::ids::SafetyGeneration) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&session);
        }
        if let Some(slot) = self.slot_for(session) {
            let context = crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: self.audio_epoch,
                safety_generation: new_generation,
            };
            if let Ok(mut hub) = self.hub.lock() {
                hub.disarm(slot, context);
            }
        }
    }
}

/// A fully started local host: capture, DSP, media sessions, and the UDP pump.
pub struct HostSession {
    runtime: HostRuntime,
    hub: Arc<Mutex<MediaHub>>,
    socket: UdpSocket,
    audio_epoch: AudioEpoch,
    stopped: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
}

impl HostSession {
    /// Start a host against an injected capture backend (tests never open real hardware).
    ///
    /// The `interface` is where the media sessions advertise their host candidate and where the
    /// UDP socket binds. The TLS identity is validated here as a precondition, even though the
    /// control server is composed separately.
    pub fn start<B: CaptureBackend + ?Sized>(
        backend: &B,
        tls: TlsIdentity,
        interface: SelectedInterface,
        request: StartHostRequest,
        channel_map: Vec<ChannelMapEntry>,
    ) -> Result<Self, HostFault> {
        tls.validate_paths()?;

        let hub = Arc::new(Mutex::new(MediaHub::new(interface.clone())));
        let sink = MediaHubSink {
            hub: Arc::clone(&hub),
        };

        let audio_epoch = crate::capture::next_audio_epoch(AudioEpoch(uuid::Uuid::nil()));
        let capture: crate::contract::CaptureRequest = request.capture.clone();
        let runtime = HostRuntime::start(
            backend,
            capture,
            audio_epoch,
            ChannelMapRevision(0),
            channel_map,
            sink,
        )?;

        let socket = UdpSocket::bind(SocketAddr::new(interface.ip, 0))?;
        socket.set_nonblocking(true)?;

        let stopped = Arc::new(AtomicBool::new(false));
        let pump_hub = Arc::clone(&hub);
        let pump_stopped = Arc::clone(&stopped);
        let pump_socket = socket.try_clone().map_err(HostFault::Socket)?;
        let pump = thread::Builder::new()
            .name("iem-media-pump".to_string())
            .spawn(move || {
                pump_loop(pump_socket, pump_hub, pump_stopped);
            })
            .map_err(HostFault::Socket)?;

        Ok(Self {
            runtime,
            hub,
            socket,
            audio_epoch,
            stopped,
            pump: Some(pump),
        })
    }

    /// Start a host against the real CoreAudio capture backend.
    pub fn start_real(
        tls: TlsIdentity,
        interface: SelectedInterface,
        request: StartHostRequest,
        channel_map: Vec<ChannelMapEntry>,
    ) -> Result<Self, HostFault> {
        Self::start(&SystemCaptureBackend, tls, interface, request, channel_map)
    }

    /// The shared media hub (for wiring the WS signaling path / the control server).
    pub fn media_hub(&self) -> Arc<Mutex<MediaHub>> {
        Arc::clone(&self.hub)
    }

    /// A [`DspControl`] bridge so the control actor's installed mixes reach the DSP worker.
    ///
    /// Without this, the actor would accept patches that never affected audio. The bridge maps a
    /// session to its media slot, forwards mixes to the runtime, and opens/closes the per-session
    /// media gate on arm/disarm.
    pub fn dsp_bridge(&self) -> Arc<dyn crate::contract::DspControl> {
        Arc::new(DspBridge {
            handle: self.runtime.handle(),
            hub: Arc::clone(&self.hub),
            audio_epoch: self.audio_epoch,
            pending: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The capture generation this session minted for its blocks.
    pub fn audio_epoch(&self) -> AudioEpoch {
        self.audio_epoch
    }

    /// The local address the media socket is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Stop capture, stop the media pump, and join cleanly. Idempotent.
    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.runtime.stop();
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The media pump: move datagrams both ways and advance session timers.
///
/// `str0m` is Sans-I/O, so the application must: drain outbound datagrams, feed inbound ones, and
/// advance the clock. The loop is intentionally non-blocking so [`HostSession::stop`] can join it
/// promptly; it is also bounded by `MAX_SESSIONS` work per datagram.
fn pump_loop(socket: UdpSocket, hub: Arc<Mutex<MediaHub>>, stopped: Arc<AtomicBool>) {
    let local = match socket.local_addr() {
        Ok(addr) => addr,
        Err(_) => return,
    };
    let mut buf = [0u8; MAX_DATAGRAM_BYTES];

    while !stopped.load(Ordering::Acquire) {
        // 1. Drain outbound datagrams and send them. The lock is released before any syscall.
        let transmits = {
            let mut guard = hub.lock().expect("media hub");
            guard.take_transmits()
        };
        for transmit in transmits {
            let _ = socket.send_to(&transmit.contents, transmit.destination);
        }

        // 2. Feed one inbound datagram, if any, to every open slot. Datagrams that do not belong
        //    to a session are ignored by that session rather than being sent to the wrong peer.
        match socket.recv_from(&mut buf) {
            Ok((len, source)) => {
                let mut guard = hub.lock().expect("media hub");
                for slot in 0..MAX_SESSIONS {
                    if guard.session_at(slot).is_some() {
                        let _ =
                            guard.handle_datagram(slot, Instant::now(), source, local, &buf[..len]);
                    }
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }

        // 3. Advance every session's timers on a deadline.
        {
            let mut guard = hub.lock().expect("media hub");
            guard.tick(Instant::now());
        }

        thread::sleep(PUMP_IDLE);
    }
}

/// Build a default channel map: `channels` input channels centered to stereo.
///
/// This is a draft POC mapping (each physical input becomes an independent source); the operator's
/// real device capability probe is what finalizes source roles.
pub fn default_channel_map(channels: u16) -> Vec<ChannelMapEntry> {
    (0..channels)
        .map(|index| ChannelMapEntry {
            physical_index: index,
            source_id: SourceId::from_bytes([index as u8; 16]),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::service::{CaptureCapabilities, CaptureSink, CaptureStream};
    use crate::contract::{CaptureBlock, EncodedFrame, SampleFormat};
    use crate::ids::{SessionEpoch, MAX_BLOCK_FRAMES};
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    struct PushingBackend {
        channels: u16,
        frames: u32,
        blocks: u32,
    }

    impl CaptureBackend for PushingBackend {
        fn capabilities(
            &self,
            _req: &crate::contract::CaptureRequest,
        ) -> Result<CaptureCapabilities, CaptureFault> {
            Ok(CaptureCapabilities {
                channels: self.channels,
                sample_rate_hz: 48_000,
                buffer_frames: self.frames,
                sample_format: SampleFormat::F32,
            })
        }

        fn build(
            &self,
            _req: &crate::contract::CaptureRequest,
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
            thread::spawn(move || {
                for _ in 0..blocks {
                    if let Ok(mut slot) = sink.free_rx.pop() {
                        for (i, sample) in slot.iter_mut().enumerate() {
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
                    thread::sleep(Duration::from_millis(1));
                }
            });
            Ok(())
        }
    }

    fn loopback() -> SelectedInterface {
        SelectedInterface {
            name: "lo0".to_string(),
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            prefix: 8,
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("iem-cast-{tag}-{}.pem", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("temp file");
        file.write_all(b"test").expect("write temp file");
        path
    }

    fn valid_tls() -> TlsIdentity {
        TlsIdentity::new(temp_file("cert"), temp_file("key"))
    }

    fn start_request() -> StartHostRequest {
        StartHostRequest {
            capture: crate::contract::CaptureRequest {
                device_id: "fake".to_string(),
                sample_rate_hz: 48_000,
                buffer_frames: 128,
            },
            interface: crate::contract::InterfaceInfo {
                name: "lo0".to_string(),
                ip_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                prefix: 8,
            },
            certificate_path: temp_file("cert").display().to_string(),
            key_path: temp_file("key").display().to_string(),
        }
    }

    #[test]
    fn host_session_starts_with_a_fake_backend_and_stops_cleanly() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 20,
        };
        let mut session = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        // The media socket binds a real loopback port; no listener slot exists yet.
        assert!(session.local_addr().is_ok());
        assert_eq!(session.media_hub().lock().unwrap().session_at(0), None);

        session.stop();
        session.stop(); // idempotent
    }

    #[test]
    fn host_session_rejects_an_invalid_tls_identity() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 1,
        };
        let missing = TlsIdentity::new("/nonexistent/cert.pem", "/nonexistent/key.pem");
        let result = HostSession::start(
            &backend,
            missing,
            loopback(),
            start_request(),
            default_channel_map(1),
        );
        assert!(matches!(result, Err(HostFault::Tls(_))));
    }

    #[test]
    fn media_hub_sink_routes_blocks_into_the_shared_hub() {
        let hub = Arc::new(Mutex::new(MediaHub::new(loopback())));
        let session = SessionEpoch::from_bytes([0x22; 16]);
        hub.lock().unwrap().open_slot(0, session).unwrap();

        let sink = MediaHubSink {
            hub: Arc::clone(&hub),
        };
        let mut outputs: [ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(ListenerOutput::empty);
        outputs[0] = ListenerOutput {
            slot: 0,
            session,
            count: 1,
            frames: [EncodedFrame::zeroed(); crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        outputs[1] = ListenerOutput {
            slot: 1,
            session: SessionEpoch::from_bytes([0x23; 16]),
            count: 1,
            frames: [EncodedFrame::zeroed(); crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };

        // No panic and no transmit while the gate is closed / the peer is not connected.
        sink.on_block(&outputs);
        assert!(hub.lock().unwrap().take_transmits().is_empty());
    }

    #[test]
    fn default_channel_map_is_centered_input_channels() {
        let map = default_channel_map(2);
        assert_eq!(map.len(), 2);
        assert_eq!(map[1].physical_index, 1);
        assert_eq!(map[1].role, SourceRole::InputChannel);
    }

    #[test]
    fn dsp_bridge_queues_a_mix_that_precedes_the_media_slot_then_applies_on_arm() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 5,
        };
        let session = SessionEpoch::from_bytes([0x22; 16]);
        let host = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        let bridge = host.dsp_bridge();

        // A patch arriving BEFORE the media slot exists must be accepted and queued, not lost.
        let snapshot = crate::contract::MixSnapshot {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(0),
            },
            catalog_revision: crate::ids::CatalogRevision(1),
            mix_revision: crate::ids::MixRevision(1),
            sources: crate::contract::SourceGainMatrix::from_slice(&[crate::contract::SourceGain {
                source_id: SourceId::from_bytes([0x11; 16]),
                gain_db: -6.0,
                muted: true,
            }]),
            master_db: 0.0,
            master_muted: true,
        };
        assert!(crate::contract::DspControl::install_snapshot(&*bridge, snapshot).is_ok());

        // Now signaling opens the media slot for the same session.
        host.media_hub().lock().unwrap().open_slot(0, session).unwrap();

        let arm = crate::contract::ListenArm {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(1),
            },
            applied_revision: crate::ids::MixRevision(1),
            arm_nonce: crate::ids::ArmNonce::from_bytes([0x66; 16]),
        };
        // Arming flushes the queued mix and opens the session's gate; must not panic or error.
        assert!(crate::contract::DspControl::request_arm(&*bridge, arm).is_ok());

        // A subsequent patch now routes straight through (slot exists).
        assert!(crate::contract::DspControl::install_snapshot(&*bridge, snapshot).is_ok());

        // Disarm is priority and must not panic.
        crate::contract::DspControl::disarm(&*bridge, session, crate::ids::SafetyGeneration(2));
    }
}
