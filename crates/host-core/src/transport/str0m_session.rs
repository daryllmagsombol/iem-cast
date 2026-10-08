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
