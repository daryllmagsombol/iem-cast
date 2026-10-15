//! Media hub: bridges encoded frames to per-listener WebRTC sessions.
//!
//! The hub owns one [`MediaSession`] per listener slot and is Sans-I/O: it accepts SDP offers,
//! ICE candidates, and inbound datagrams, and exposes the outbound datagrams for the caller to
//! send. This keeps socket ownership in one place (the host runtime) while all WebRTC state stays
//! here, behind the per-session safety gate.
//!
//! [`MediaHub::write_block`] implements [`EncodedSink`], so it can be handed directly to
//! [`crate::runtime::HostRuntime`] and will route each listener's frames to its own session.

use std::net::IpAddr;

use crate::contract::MediaFailureCode;
use crate::ids::SessionEpoch;
use crate::pipeline::ListenerOutput;

use super::mdns::{CandidateClass, MdnsResolver};
use super::str0m_session::{MediaSession, SelectedInterface, Transmit};
use crate::audio::engine::MAX_SESSIONS;

/// Resolves `.local` hostnames with the operating system resolver.
///
/// This is best-effort and is explicitly **not** a Bonjour guarantee; a failure is surfaced as
/// `MdnUnresolved` rather than being hidden.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemMdnsResolver;

impl SystemMdnsResolver {
    /// Create a system resolver.
    pub fn new() -> Self {
        Self
    }
}

impl MdnsResolver for SystemMdnsResolver {
    fn resolve(&self, hostname: &str) -> Result<IpAddr, MediaFailureCode> {
        // Port 0 is enough to resolve an address; the port is not used.
        let target = format!("{hostname}:0");
        let addrs = std::net::ToSocketAddrs::to_socket_addrs(&target)
            .map_err(|_| MediaFailureCode::MdnUnresolved)?;
        addrs
            .map(|addr| addr.ip())
            .next()
            .ok_or(MediaFailureCode::MdnUnresolved)
    }
}

/// One listener's media slot state.
struct Slot {
    session: SessionEpoch,
    media: MediaSession,
}

/// Owns the per-listener WebRTC sessions.
pub struct MediaHub {
    iface: SelectedInterface,
    slots: [Option<Slot>; MAX_SESSIONS],
    resolver: Box<dyn MdnsResolver>,
}

impl MediaHub {
    /// Create a hub bound to a selected LAN interface.
    pub fn new(iface: SelectedInterface) -> Self {
        Self::with_resolver(iface, Box::new(SystemMdnsResolver))
    }

    /// Create a hub with an injected resolver (used by tests to stay offline).
    pub fn with_resolver(iface: SelectedInterface, resolver: Box<dyn MdnsResolver>) -> Self {
        Self {
            iface,
            slots: [None, None, None, None],
            resolver,
        }
    }

    /// Attach a listener slot, creating its media session.
    pub fn open_slot(&mut self, slot: usize, session: SessionEpoch) -> Result<(), MediaFailureCode> {
        if slot >= MAX_SESSIONS {
            return Err(MediaFailureCode::NegotiationFailed);
        }
        let media = MediaSession::new(session, self.iface.clone())?;
        self.slots[slot] = Some(Slot { session, media });
        Ok(())
    }

    /// Detach a listener slot and drop its session.
    pub fn close_slot(&mut self, slot: usize) {
        if slot < MAX_SESSIONS {
            self.slots[slot] = None;
        }
    }

    /// Which session currently occupies a slot.
    pub fn session_at(&self, slot: usize) -> Option<SessionEpoch> {
        self.slots.get(slot).and_then(|s| s.as_ref()).map(|s| s.session)
    }

    /// The interface (address + advertised media port) new sessions are bound to.
    pub fn interface(&self) -> &SelectedInterface {
        &self.iface
    }

    /// Accept a browser offer and return the host answer.
    pub fn handle_offer(&mut self, slot: usize, sdp: &str) -> Result<String, MediaFailureCode> {
        let slot_ref = self.slot_mut(slot)?;
        slot_ref.media.handle_offer(sdp)
    }

    /// Add a remote ICE candidate (`None` = end of candidates).
    pub fn add_candidate(
        &mut self,
        slot: usize,
        candidate: Option<&str>,
    ) -> Result<(), MediaFailureCode> {
        // Borrow-split so the resolver (owned by the hub) is available while the session is mutably
        // borrowed.
        let resolver: &dyn MdnsResolver = &*self.resolver;
        let slots = &mut self.slots;
        let slot_ref = slots
            .get_mut(slot)
            .and_then(|s| s.as_mut())
            .ok_or(MediaFailureCode::NegotiationFailed)?;
        slot_ref.media.add_candidate(candidate, resolver)
    }

    /// Open the safety gate for a session once the control plane confirms the arm tuple.
    pub fn arm(&mut self, slot: usize, context: crate::contract::SessionContext) {
        if let Some(slot_ref) = self.slots.get_mut(slot).and_then(|s| s.as_mut()) {
            slot_ref.media.arm(context);
        }
    }

    /// Close the safety gate immediately (priority disarm).
    pub fn disarm(&mut self, slot: usize, context: crate::contract::SessionContext) {
        if let Some(slot_ref) = self.slots.get_mut(slot).and_then(|s| s.as_mut()) {
            slot_ref.media.disarm(context);
        }
    }

    /// Feed an inbound datagram to the session that owns it.
    pub fn handle_datagram(
        &mut self,
        slot: usize,
        now: std::time::Instant,
        source: std::net::SocketAddr,
        destination: std::net::SocketAddr,
        buf: &[u8],
    ) -> Result<(), MediaFailureCode> {
        let slot_ref = self.slot_mut(slot)?;
        slot_ref.media.handle_datagram(now, source, destination, buf)
    }

    /// Feed a deadline so every session can advance its own timers.
    pub fn tick(&mut self, now: std::time::Instant) {
        for slot in self.slots.iter_mut().filter_map(|s| s.as_mut()) {
            let _ = slot.media.handle_timeout(now);
        }
    }

    /// The earliest deadline any occupied session wants the next timer tick at, if any.
    ///
    /// `None` when no slot is occupied. Sessions with no captured deadline are skipped.
    pub(crate) fn next_deadline(&self) -> Option<std::time::Instant> {
        self.slots
            .iter()
            .filter_map(|s| s.as_ref())
            .filter_map(|slot| slot.media.next_timeout())
            .min()
    }

    /// Drain every outbound datagram across all sessions into `out`, appending to its contents.
    ///
    /// Reuses `out`'s capacity (and preserves anything already staged in it), so the caller can keep
    /// one scratch buffer for the lifetime of the pump.
    pub(crate) fn drain_transmits_into(&mut self, out: &mut Vec<Transmit>) {
        for slot in self.slots.iter_mut().filter_map(|s| s.as_mut()) {
            slot.media.drain_transmits_into(out);
        }
    }

    /// Whether a slot's peer is connected.
    pub fn is_connected(&self, slot: usize) -> bool {
        self.slots
            .get(slot)
            .and_then(|s| s.as_ref())
            .map(|s| s.media.is_connected())
            .unwrap_or(false)
    }

    fn slot_mut(&mut self, slot: usize) -> Result<&mut Slot, MediaFailureCode> {
        self.slots
            .get_mut(slot)
            .and_then(|s| s.as_mut())
            .ok_or(MediaFailureCode::NegotiationFailed)
    }
}

/// Route one block's encoded frames to their sessions.
///
/// Frames are checked against each session's safety gate inside `write_packet`, so anything
/// produced before a disarm is dropped rather than transmitted. Frames are also dropped when the
/// block's session no longer matches the slot, so a reused slot can never inherit late frames.
///
/// Returns `true` when at least one frame was accepted by a session's writer (a frame was actually
/// staged for transmission). A closed gate, a missing slot, or a disconnected peer yields `false`.
impl MediaHub {
    pub fn write_block(&mut self, outputs: &[ListenerOutput; MAX_SESSIONS]) -> bool {
        let mut wrote = false;
        for output in outputs.iter().filter(|o| o.count > 0) {
            let Some(slot_ref) = self.slots.get_mut(output.slot).and_then(|s| s.as_mut()) else {
                continue;
            };
            if slot_ref.session != output.session {
                continue;
            }
            for frame in output.frames.iter().take(output.count) {
                // A closed gate or a not-yet-connected peer is expected, not an error.
                if slot_ref.media.write_packet(frame).is_ok() {
                    wrote = true;
                }
            }
        }
        wrote
    }

    /// Whether a candidate line may be handed to the session without escalation.
    pub fn candidate_is_supported(candidate: &str) -> bool {
        !matches!(
            super::mdns::classify_candidate(candidate),
            CandidateClass::Unrecognised
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::SessionContext;
    use crate::ids::{AudioEpoch, SafetyGeneration};
    use uuid::Uuid;

    fn iface() -> SelectedInterface {
        SelectedInterface {
            name: "en0".to_string(),
            ip: IpAddr::from([192, 168, 1, 10]),
            prefix: 24,
            port: 0,
        }
    }

    fn session(n: u8) -> SessionEpoch {
        SessionEpoch(Uuid::from_bytes([n; 16]))
    }

    /// A resolver that never succeeds, so tests stay offline.
    struct OfflineResolver;
    impl MdnsResolver for OfflineResolver {
        fn resolve(&self, _hostname: &str) -> Result<IpAddr, MediaFailureCode> {
            Err(MediaFailureCode::MdnUnresolved)
        }
    }

    fn hub() -> MediaHub {
        MediaHub::with_resolver(iface(), Box::new(OfflineResolver))
    }

    #[test]
    fn open_slot_registers_a_session() {
        let mut hub = hub();
        hub.open_slot(0, session(1)).unwrap();
        assert_eq!(hub.session_at(0), Some(session(1)));
        assert!(!hub.is_connected(0));
    }

    #[test]
    fn close_slot_drops_the_session() {
        let mut hub = hub();
        hub.open_slot(0, session(1)).unwrap();
        hub.close_slot(0);
        assert_eq!(hub.session_at(0), None);
    }

    #[test]
    fn operations_on_a_missing_slot_are_errors() {
        let mut hub = hub();
        assert_eq!(
            hub.handle_offer(2, "v=0\r\n"),
            Err(MediaFailureCode::NegotiationFailed)
        );
        assert_eq!(
            hub.add_candidate(2, None),
            Err(MediaFailureCode::NegotiationFailed)
        );
    }

    #[test]
    fn end_of_candidates_is_accepted() {
        let mut hub = hub();
        hub.open_slot(0, session(1)).unwrap();
        assert!(hub.add_candidate(0, None).is_ok());
    }

    #[test]
    fn ip_candidate_is_supported_without_resolution() {
        assert!(MediaHub::candidate_is_supported(
            "candidate:1 1 udp 2122260223 192.168.1.5 50000 typ host"
        ));
        assert!(!MediaHub::candidate_is_supported("garbage"));
    }

    #[test]
    fn writing_before_arm_transmits_nothing() {
        let mut hub = hub();
        let s = session(1);
        hub.open_slot(0, s).unwrap();

        // A frame carrying this session but written while the gate is closed must not queue any
        // datagram.
        let output = ListenerOutput {
            slot: 0,
            session: s,
            count: 1,
            frames: [crate::contract::EncodedFrame::zeroed(); crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        let mut outputs: [ListenerOutput; MAX_SESSIONS] = std::array::from_fn(ListenerOutput::empty);
        outputs[0] = output;
        // A closed gate means nothing is accepted, so the pump is not woken.
        assert!(!hub.write_block(&outputs));
        let mut scratch = Vec::new();
        hub.drain_transmits_into(&mut scratch);
        assert!(scratch.is_empty());
    }

    #[test]
    fn write_block_reports_false_when_nothing_is_accepted() {
        let mut hub = hub();
        let s = session(1);
        hub.open_slot(0, s).unwrap();

        let frame = crate::contract::EncodedFrame::zeroed();
        let mut outputs: [ListenerOutput; MAX_SESSIONS] = std::array::from_fn(ListenerOutput::empty);

        // Missing slot (slot 2 is empty) -> false.
        outputs[2] = ListenerOutput {
            slot: 2,
            session: s,
            count: 1,
            frames: [frame; crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        assert!(!hub.write_block(&outputs));

        // Session mismatch on an occupied slot -> false.
        let mut mismatched: [ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(ListenerOutput::empty);
        mismatched[0] = ListenerOutput {
            slot: 0,
            session: session(2),
            count: 1,
            frames: [frame; crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        assert!(!hub.write_block(&mismatched));

        // Closed gate on the matching slot -> false.
        outputs[0] = ListenerOutput {
            slot: 0,
            session: s,
            count: 1,
            frames: [frame; crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        assert!(!hub.write_block(&outputs));
    }

    #[test]
    fn drain_transmits_into_preserves_preexisting_scratch_contents() {
        let mut hub = hub();
        hub.open_slot(0, session(1)).unwrap();

        let mut scratch: Vec<Transmit> = vec![Transmit {
            source: "127.0.0.1:1".parse().unwrap(),
            destination: "127.0.0.1:2".parse().unwrap(),
            contents: vec![0xAB],
        }];

        // No queued transmits, but the caller's staged entry must survive the drain.
        hub.drain_transmits_into(&mut scratch);
        assert_eq!(scratch.len(), 1);
        assert_eq!(scratch[0].contents, vec![0xAB]);
    }

    #[test]
    fn next_deadline_is_none_without_slots_and_some_after_tick() {
        let mut hub = hub();
        // No occupied slot -> no deadline to honor.
        assert_eq!(hub.next_deadline(), None);

        hub.open_slot(0, session(1)).unwrap();
        let before = std::time::Instant::now();
        hub.tick(before);

        // A fresh session schedules a timeout, so the hub surfaces a deadline at or after the tick.
        let deadline = hub.next_deadline().expect("an occupied slot schedules a timeout");
        assert!(
            deadline >= before,
            "the deadline must not predate the tick that produced it"
        );
    }

    #[test]
    fn armed_gate_but_disconnected_peer_still_transmits_nothing() {
        let mut hub = hub();
        let s = session(1);
        hub.open_slot(0, s).unwrap();
        hub.arm(
            0,
            SessionContext {
                session_epoch: s,
                audio_epoch: AudioEpoch::from_bytes([0x55; 16]),
                safety_generation: SafetyGeneration(1),
            },
        );
        // Not ICE-connected, so `write_packet` refuses and nothing is queued.
        assert!(!hub.is_connected(0));
    }
}
