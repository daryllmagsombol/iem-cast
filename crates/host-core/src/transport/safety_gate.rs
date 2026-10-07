//! Backend safety gate for outbound media.
//!
//! Encoded frames carry the exact [`SessionContext`] (session, audio epoch, and safety
//! generation) that produced them. Before a packet is handed to the network, the gate checks
//! that context against the session's current generation. A frame produced before a disarm must
//! never be transmitted, and no stale queue is retained after a disarm.

use crate::contract::{EncodedFrame, SessionContext};
use crate::ids::SessionEpoch;

/// Whether a frame may be written to the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GateDecision {
    /// The frame matches the current generation; transmit it.
    Transmit,
    /// The frame is stale (different session, epoch, or generation); drop it.
    Drop,
}

/// Per-session outbound safety gate.
#[derive(Clone, Copy, Debug)]
pub struct SafetyGate {
    session: SessionEpoch,
    context: SessionContext,
    armed: bool,
}

impl SafetyGate {
    /// Create a gate for a session. It starts closed (unarmed) so nothing is sent until an arm
    /// confirmation installs a generation.
    pub fn new(session: SessionEpoch) -> Self {
        Self {
            session,
            context: SessionContext::NONE,
            armed: false,
        }
    }

    /// Install the confirmed context and open the gate.
    pub fn arm(&mut self, context: SessionContext) {
        if context.session_epoch == self.session {
            self.context = context;
            self.armed = true;
        }
    }

    /// Close the gate immediately and advance to the given generation.
    ///
    /// Called on a priority disarm. Any queued frame carrying an older generation is dropped.
    pub fn disarm(&mut self, new_context: SessionContext) {
        self.context = new_context;
        self.armed = false;
    }

    /// Whether the gate is currently open.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Decide whether a frame may be transmitted.
    ///
    /// A frame is transmittable only when the gate is armed and the frame's entire context —
    /// session, audio epoch, and safety generation — matches the gate exactly.
    pub fn decide(&self, frame: &EncodedFrame) -> GateDecision {
        let ctx = frame.context;
        if !self.armed {
            return GateDecision::Drop;
        }
        if ctx.session_epoch != self.session
            || ctx.audio_epoch != self.context.audio_epoch
            || ctx.safety_generation != self.context.safety_generation
        {
            return GateDecision::Drop;
        }
        GateDecision::Transmit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::BoundedPacket;
    use crate::ids::{AudioEpoch, SafetyGeneration, SessionEpoch};
    use uuid::Uuid;

    fn session(n: u8) -> SessionEpoch {
        SessionEpoch(Uuid::from_bytes([n; 16]))
    }

    fn ctx(session: SessionEpoch, gen: u64) -> SessionContext {
        SessionContext {
            session_epoch: session,
            audio_epoch: AudioEpoch(Uuid::from_bytes([9; 16])),
            safety_generation: SafetyGeneration(gen),
        }
    }

    fn frame(context: SessionContext) -> EncodedFrame {
        EncodedFrame {
            context,
            start_sample: 0,
            frame_count: 120,
            enqueue_instant: std::time::Instant::now(),
            packet: BoundedPacket::empty(),
        }
    }

    #[test]
    fn gate_is_closed_until_armed() {
        let s = session(1);
        let gate = SafetyGate::new(s);
        assert_eq!(gate.decide(&frame(ctx(s, 1))), GateDecision::Drop);
    }

    #[test]
    fn armed_gate_transmits_matching_context() {
        let s = session(1);
        let mut gate = SafetyGate::new(s);
        gate.arm(ctx(s, 1));
        assert_eq!(gate.decide(&frame(ctx(s, 1))), GateDecision::Transmit);
    }

    #[test]
    fn stale_generation_dropped_after_disarm() {
        let s = session(1);
        let mut gate = SafetyGate::new(s);
        gate.arm(ctx(s, 1));
        gate.disarm(ctx(s, 2));
        assert_eq!(gate.decide(&frame(ctx(s, 1))), GateDecision::Drop);
        assert!(!gate.is_armed());
    }

    #[test]
    fn other_session_frame_is_dropped() {
        let s = session(1);
        let mut gate = SafetyGate::new(s);
        gate.arm(ctx(s, 1));
        assert_eq!(gate.decide(&frame(ctx(session(2), 1))), GateDecision::Drop);
    }
}
