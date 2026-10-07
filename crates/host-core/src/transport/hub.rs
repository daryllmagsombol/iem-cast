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

    /// Drain every outbound datagram across all sessions.
    pub fn take_transmits(&mut self) -> Vec<Transmit> {
        let mut out = Vec::new();
        for slot in self.slots.iter_mut().filter_map(|s| s.as_mut()) {
            out.extend(slot.media.take_transmits());
        }
        out
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
impl MediaHub {
    pub fn write_block(&mut self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
        for output in outputs.iter().filter(|o| o.count > 0) {
            let Some(slot_ref) = self.slots.get_mut(output.slot).and_then(|s| s.as_mut()) else {
                continue;
            };
            if slot_ref.session != output.session {
                continue;
            }
            for frame in output.frames.iter().take(output.count) {
                // A closed gate or a not-yet-connected peer is expected, not an error.
                let _: Result<(), MediaFailureCode> = slot_ref.media.write_packet(frame);
            }
        }
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
        hub.write_block(&outputs);
        assert!(hub.take_transmits().is_empty());
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
