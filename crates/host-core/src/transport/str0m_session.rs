//! Per-listener WebRTC media session built on `str0m`.
//!
//! The session is a Sans-I/O `str0m::Rtc` plus the POC's negotiation and safety rules. The
//! application owns the UDP socket and drives the canonical run loop:
//!
//! 1. feed one input (`handle_input` with a deadline or a received datagram),
//! 2. drain `poll_output()` until `Output::Timeout`,
//! 3. send every `Output::Transmit`, translate every `Output::Event`.
//!
//! `str0m` drops media written before ICE connects, so the session refuses to write until the
//! peer reports `Connected`, and the [`SafetyGate`] independently refuses any frame whose
//! session/epoch/generation does not match the armed context.

use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use str0m::change::SdpOffer;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

use crate::contract::{EncodedFrame, MediaEvent, MediaFailureCode};
use crate::ids::SessionEpoch;

use super::mdns::{resolve_candidate, MdnsLimits, MdnsResolver};
use super::safety_gate::{GateDecision, SafetyGate};
use super::sdp::{negotiate_stereo, validate_offer};

/// The LAN interface a session binds its host candidate to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SelectedInterface {
    pub name: String,
    pub ip: IpAddr,
    pub prefix: u8,
    /// The UDP port the host advertises and receives WebRTC media on.
    ///
    /// This must be the *actual* bound socket port. Advertising `0` hands the phone an
    /// unreachable candidate and ICE never connects.
    pub port: u16,
}

impl SelectedInterface {
    fn host_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }
}

/// One outbound datagram the application must send.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Transmit {
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub contents: Vec<u8>,
}

/// A per-listener media session.
pub struct MediaSession {
    rtc: Rtc,
    session: SessionEpoch,
    iface: SelectedInterface,
    mid: Option<Mid>,
    pt: Option<Pt>,
    gate: SafetyGate,
    limits: MdnsLimits,
    pending_transmits: Vec<Transmit>,
    pending_events: Vec<MediaEvent>,
    connected: bool,
    failed: bool,
}

impl MediaSession {
    /// Create a session and register the selected interface's host candidate.
    pub fn new(session: SessionEpoch, iface: SelectedInterface) -> Result<Self, MediaFailureCode> {
        let mut rtc = Rtc::builder().set_rtp_mode(false).build(Instant::now());
        let candidate = Candidate::host(iface.host_addr(), "udp")
            .map_err(|_| MediaFailureCode::IceFailed)?;
        rtc.add_local_candidate(candidate);
        Ok(Self {
            rtc,
            session,
            iface,
            mid: None,
            pt: None,
            gate: SafetyGate::new(session),
            limits: MdnsLimits::default(),
            pending_transmits: Vec::new(),
            pending_events: Vec::new(),
            connected: false,
            failed: false,
        })
    }

    /// Accept the browser's receive-only audio offer and return the send-only answer.
    ///
    /// The offer is validated with pure rules first (exactly one audio section, `recvonly`,
    /// stereo Opus), so a malformed or mono-only offer is refused with a typed failure rather
    /// than silently mis-negotiated.
    pub fn handle_offer(&mut self, sdp: &str) -> Result<String, MediaFailureCode> {
        let parsed = validate_offer(sdp)?;
        let _pt_value = negotiate_stereo(&parsed)?;

        let offer = SdpOffer::from_sdp_string(sdp).map_err(|_| MediaFailureCode::NegotiationFailed)?;
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|_| MediaFailureCode::NegotiationFailed)?;

        // Drain outputs produced by the negotiation.
        self.drain_outputs()?;

        Ok(answer.to_sdp_string())
    }

    /// Add a remote ICE candidate. `None` signals end-of-candidates and is not an error.
    pub fn add_candidate(
        &mut self,
        candidate: Option<&str>,
        resolver: &dyn MdnsResolver,
    ) -> Result<(), MediaFailureCode> {
        let Some(raw) = candidate else {
            return Ok(());
        };
        let resolved = resolve_candidate(raw, resolver, &mut self.limits)?;
        let parsed = Candidate::from_sdp_string(&resolved).map_err(|_| MediaFailureCode::MdnUnresolved)?;
        self.rtc.add_remote_candidate(parsed);
        self.drain_outputs()?;
        Ok(())
    }

    /// Write one encoded frame, subject to the safety gate. The mid/payload type come from the
    /// negotiated media section; no payload type is fabricated.
    pub fn write_packet(&mut self, frame: &EncodedFrame) -> Result<(), MediaFailureCode> {
        if self.gate.decide(frame) != GateDecision::Transmit {
            return Err(MediaFailureCode::PeerDisconnected);
        }
        if !self.connected {
            return Err(MediaFailureCode::IceFailed);
        }
        let mid = self.mid.ok_or(MediaFailureCode::NegotiationFailed)?;
        let pt = self.pt.ok_or(MediaFailureCode::NegotiationFailed)?;
        let len = frame.packet.len as usize;
        let data = frame.packet.bytes[..len].to_vec();
        let frequency = Frequency::new(48_000).ok_or(MediaFailureCode::NegotiationFailed)?;
        let rtp_time = MediaTime::new(frame.start_sample, frequency);

        let Some(writer) = self.rtc.writer(mid) else {
            return Err(MediaFailureCode::NegotiationFailed);
        };
        writer
            .write(pt, Instant::now(), rtp_time, data)
            .map_err(|_| MediaFailureCode::PeerDisconnected)?;
        self.drain_outputs()?;
        Ok(())
    }

    /// Arm the session with the confirmed context. Until armed, the safety gate refuses frames.
    pub fn arm(&mut self, context: crate::contract::SessionContext) {
        self.gate.arm(context);
    }

    /// Priority disarm: close the safety gate immediately and advance the generation.
    pub fn disarm(&mut self, context: crate::contract::SessionContext) {
        self.gate.disarm(context);
    }

    /// Feed a datagram received from the socket for this session.
    pub fn handle_datagram(
        &mut self,
        now: Instant,
        source: SocketAddr,
        destination: SocketAddr,
        buf: &[u8],
    ) -> Result<(), MediaFailureCode> {
        let receive = str0m::net::Receive::new(str0m::net::Protocol::Udp, source, destination, buf)
            .map_err(|_| MediaFailureCode::PeerDisconnected)?;
        self.rtc
            .handle_input(Input::Receive(now, receive))
            .map_err(|_| MediaFailureCode::PeerDisconnected)?;
        self.drain_outputs()
    }

    /// Feed a deadline so `str0m` can advance its own timers.
    pub fn handle_timeout(&mut self, now: Instant) -> Result<(), MediaFailureCode> {
        self.rtc
            .handle_input(Input::Timeout(now))
            .map_err(|_| MediaFailureCode::IceFailed)?;
        self.drain_outputs()
    }

    /// Take queued outbound datagrams for the application to send.
    pub fn take_transmits(&mut self) -> Vec<Transmit> {
        std::mem::take(&mut self.pending_transmits)
    }

    /// Take queued media events for the control plane.
    pub fn take_events(&mut self) -> Vec<MediaEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Whether the peer is ICE/DTLS connected and media may be written.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// The selected interface this session was created for.
    pub fn interface(&self) -> &SelectedInterface {
        &self.iface
    }

    /// Drain `poll_output()` until `Output::Timeout`, collecting transmits and events.
    ///
    /// This is the single-mutation invariant: every mutation is followed by a full drain.
    fn drain_outputs(&mut self) -> Result<(), MediaFailureCode> {
        loop {
            let output = self
                .rtc
                .poll_output()
                .map_err(|_| MediaFailureCode::PeerDisconnected)?;
            match output {
                Output::Timeout(_) => return Ok(()),
                Output::Transmit(t) => {
                    self.pending_transmits.push(Transmit {
                        source: t.source,
                        destination: t.destination,
                        contents: t.contents.to_vec(),
                    });
                }
                Output::Event(event) => self.handle_event(event),
            }
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Connected => {
                self.connected = true;
                self.pending_events.push(MediaEvent::Readiness {
                    session_epoch: self.session,
                    ready: true,
                });
            }
            Event::IceConnectionStateChange(state) => match state {
                IceConnectionState::Connected | IceConnectionState::Completed => {
                    self.connected = true;
                    self.pending_events.push(MediaEvent::Readiness {
                        session_epoch: self.session,
                        ready: true,
                    });
                }
                IceConnectionState::Disconnected => {
                    self.connected = false;
                    self.pending_events.push(MediaEvent::Failure {
                        session_epoch: self.session,
                        code: MediaFailureCode::PeerDisconnected,
                    });
                }
                _ => {}
            },
            Event::MediaAdded(added) => {
                if added.kind == MediaKind::Audio && added.direction == Direction::SendOnly {
                    self.mid = Some(added.mid);
                }
                // Learn the negotiated payload type for this mid from the writer.
                if let Some(mid) = self.mid {
                    if let Some(writer) = self.rtc.writer(mid) {
                        if let Some(params) = writer.payload_params().next() {
                            self.pt = Some(params.pt());
                        }
                    }
                }
            }
            Event::Closed => {
                self.connected = false;
                if !self.failed {
                    self.failed = true;
                    self.pending_events.push(MediaEvent::Failure {
                        session_epoch: self.session,
                        code: MediaFailureCode::PeerDisconnected,
                    });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a real receive-only audio offer the way a browser would, using a second str0m
    /// instance. This gives us a genuine SDP offer instead of a hand-written fixture, so the
    /// answer we assert on is produced through the same code path a phone triggers.
    fn browser_like_offer() -> String {
        use str0m::media::{Direction, MediaKind};

        let mut rtc = Rtc::builder().set_rtp_mode(false).build(Instant::now());
        // A browser's own host candidate, present in the offer.
        rtc.add_local_candidate(
            Candidate::host(SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 50000), "udp")
                .expect("browser candidate"),
        );
        let mut changes = rtc.sdp_api();
        changes.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        let (offer, _pending) = changes.apply().expect("browser creates an offer");
        offer.to_sdp_string()
    }

    #[test]
    fn answer_advertises_the_configured_host_candidate_port() {
        // Regression: the answer used to advertise the host candidate at port 0, so a phone that
        // successfully paired and connected over WSS still failed ICE with "WebRTC connection
        // failed". The advertised port must be the real media socket port.
        let iface = SelectedInterface {
            name: "en0".to_string(),
            ip: IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10)),
            prefix: 24,
            port: 45123,
        };
        let mut session = MediaSession::new(SessionEpoch(uuid::Uuid::new_v4()), iface)
            .expect("session is created");

        let answer = session
            .handle_offer(&browser_like_offer())
            .expect("a real offer is accepted");

        // The host must advertise a concrete, reachable UDP candidate on the configured port.
        assert!(
            answer.contains("a=candidate:"),
            "answer must carry an ICE candidate:\n{answer}"
        );
        assert!(
            answer.contains("192.168.1.10") && answer.contains("45123"),
            "answer must advertise the configured address and REAL port, not port 0:\n{answer}"
        );
        assert!(
            !answer.contains("192.168.1.10 0 "),
            "answer must never advertise the media port as 0"
        );
        // The host is a non-trickle answerer, so the answer is send-only.
        assert!(answer.contains("a=sendonly"), "answer must be send-only:\n{answer}");
    }

    /// Drive one simulated datagram/timer tick between the production [`MediaSession`] and a
    /// peer `str0m::Rtc`. No sockets or real network are involved: every `Transmit` is delivered
    /// straight into the other peer, exactly as the application's UDP pump would.
    fn pump_once(
        host: &mut MediaSession,
        browser: &mut Rtc,
        now: Instant,
        browser_events: &mut Vec<Event>,
    ) {
        use str0m::net::{Protocol, Receive};

        host.handle_timeout(now).expect("host timer");
        browser
            .handle_input(Input::Timeout(now))
            .expect("browser timer");

        // Host -> browser.
        for t in host.take_transmits() {
            let receive = Receive::new(Protocol::Udp, t.source, t.destination, &t.contents)
                .expect("host datagram parses");
            browser
                .handle_input(Input::Receive(now, receive))
                .expect("browser receives");
        }

        // Browser -> host.
        loop {
            match browser.poll_output().expect("browser poll") {
                Output::Timeout(_) => break,
                Output::Transmit(t) => {
                    host.handle_datagram(now, t.source, t.destination, &t.contents)
                        .expect("host receives");
                }
                Output::Event(e) => browser_events.push(e),
            }
        }
    }

    #[test]
    fn negotiated_session_connects_then_learns_mid_and_delivers_opus() {
        // End-to-end regression via a real two-peer ICE/DTLS handshake, driven by a deterministic
        // in-memory datagram/timer pump. It proves:
        //   1. `handle_offer` does NOT expose a mid/pt before the peer is connected. str0m withholds
        //      media events until SRTP is ready (`poll_event` gates on `ready_for_srtp`, installed
        //      str0m 0.24.1 session.rs). The earlier test that asserted mid/pt directly after
        //      `handle_offer` was therefore invalid, not evidence of a transport defect.
        //   2. Once connected, draining outputs yields the sendonly `MediaAdded`, so mid and the
        //      negotiated payload type are learned.
        //   3. A real Opus-encoded frame written through the armed safety gate reaches the
        //      receiving peer as `MediaData`.
        use std::time::Duration;

        use str0m::change::SdpAnswer;

        use crate::audio::CODEC_FRAME_FRAMES;
        use crate::contract::{EncodedFrame, SessionContext, StereoFrame};
        use crate::encoder::OpusWorker;
        use crate::ids::{AudioEpoch, SafetyGeneration};

        let session_epoch = SessionEpoch(uuid::Uuid::new_v4());
        let iface = SelectedInterface {
            name: "en0".to_string(),
            ip: IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 10)),
            prefix: 24,
            port: 45123,
        };
        let mut host = MediaSession::new(session_epoch, iface).expect("session is created");

        // A browser-like peer: same crate, genuine SDP, its own host candidate.
        let browser_addr =
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20)), 50_000);
        let mut browser = Rtc::builder().set_rtp_mode(false).build(Instant::now());
        browser
            .add_local_candidate(Candidate::host(browser_addr, "udp").expect("browser candidate"));

        let mut changes = browser.sdp_api();
        changes.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
        let (offer, pending) = changes.apply().expect("browser creates an offer");

        let answer_sdp = host
            .handle_offer(&offer.to_sdp_string())
            .expect("a real offer is accepted");

        // Corrected assertion: nothing media-related is learned until the peer is connected.
        assert!(
            host.mid.is_none(),
            "no media mid may be learned before the peer is connected"
        );
        assert!(
            host.pt.is_none(),
            "no payload type may be learned before the peer is connected"
        );

        let answer = SdpAnswer::from_sdp_string(&answer_sdp).expect("host answer parses");
        browser
            .sdp_api()
            .accept_answer(pending, answer)
            .expect("browser accepts the answer");

        // Pump until both peers connect and the host has drained the sendonly MediaAdded.
        let mut now = Instant::now();
        let mut browser_events: Vec<Event> = Vec::new();
        for _ in 0..5_000 {
            now += Duration::from_millis(2);
            pump_once(&mut host, &mut browser, now, &mut browser_events);
            if host.is_connected() && browser.is_connected() && host.mid.is_some() {
                break;
            }
        }

        assert!(host.is_connected(), "host must reach ICE/DTLS connected");
        assert!(browser.is_connected(), "browser peer must reach connected");
        assert!(
            host.mid.is_some(),
            "mid must be learned only after connected outputs are drained"
        );
        assert!(
            host.pt.is_some(),
            "negotiated payload type must be learned once connected"
        );
        assert!(
            host.take_events()
                .iter()
                .any(|e| matches!(e, MediaEvent::Readiness { ready: true, .. })),
            "the host must publish a readiness event on connect"
        );

        // One real encoded Opus frame through the armed safety gate.
        let context = SessionContext {
            session_epoch,
            audio_epoch: AudioEpoch::from_bytes([0x55; 16]),
            safety_generation: SafetyGeneration(1),
        };
        host.arm(context);

        let mut stereo = StereoFrame::zeroed(context);
        stereo.start_sample = 0;
        stereo.frame_count = CODEC_FRAME_FRAMES;
        for n in 0..CODEC_FRAME_FRAMES as usize {
            let value = (n as f32 / CODEC_FRAME_FRAMES as f32) * 0.25;
            stereo.pcm[n * 2] = value;
            stereo.pcm[n * 2 + 1] = -value;
        }
        let mut encoded = EncodedFrame::zeroed();
        OpusWorker::new()
            .expect("opus worker")
            .encode(&stereo, &mut encoded)
            .expect("opus encode");
        assert!(encoded.packet.len > 0, "encoder must produce a payload");

        host.write_packet(&encoded)
            .expect("an armed, connected write is accepted");

        // Flush the queued payload into a datagram and deliver it to the receiver.
        for _ in 0..200 {
            now += Duration::from_millis(2);
            pump_once(&mut host, &mut browser, now, &mut browser_events);
            if browser_events
                .iter()
                .any(|e| matches!(e, Event::MediaData(_)))
            {
                break;
            }
        }

        let media = browser_events
            .iter()
            .find_map(|e| match e {
                Event::MediaData(d) => Some(d),
                _ => None,
            })
            .expect("the receiving peer must surface the sent frame as MediaData");
        assert_eq!(
            media.mid,
            host.mid.expect("host mid"),
            "mid must match the sendonly mid"
        );
        assert!(
            !media.data.is_empty(),
            "the delivered Opus frame must carry payload bytes"
        );
    }
}
