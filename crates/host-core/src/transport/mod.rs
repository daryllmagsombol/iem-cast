//! WebRTC / media transport boundary (Lane C).
//!
//! The transport is split into offline-testable pure logic and a thin `str0m` session:
//! - [`sdp`] validates the offer and adapts Opus format parameters without rewriting SDP.
//! - [`mdns`] resolves bounded `.local` candidates before they reach the session.
//! - [`safety_gate`] refuses stale frames before they reach the network.
//! - [`str0m_session`] is the Sans-I/O session the application drives with datagrams/deadlines.

pub mod hub;
pub mod mdns;
pub mod safety_gate;
pub mod sdp;
pub mod str0m_session;

pub use hub::{MediaHub, SystemMdnsResolver};
pub use mdns::{classify_candidate, resolve_candidate, CandidateClass, MdnsLimits, MdnsResolver};
pub use safety_gate::{GateDecision, SafetyGate};
pub use sdp::{
    is_mono_only_fmtp, negotiate_stereo, negotiated_opus_pt, parse_offer, validate_offer, ParsedOffer,
};
pub use str0m_session::{MediaSession, SelectedInterface, Transmit};
