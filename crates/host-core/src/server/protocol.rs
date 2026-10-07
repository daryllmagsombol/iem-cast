//! Server-to-client control messages.
//!
//! [`ServerMessage`] is defined in Lane B's own module (not the frozen `contract.rs`). It is a
//! typed server event that serializes into a v1 envelope. The control actor returns these from
//! [`crate::control::ControlActor::on_audio_event`] after validating the installed revision and
//! session context.

use serde::Serialize;

use crate::contract::{
    ControlError, EnvelopeV1, ListenArmed, MixAck, MixApplied, RtcAnswer, RtcCandidate,
};
use crate::ids::{HostEpoch, RequestId, SessionEpoch};

/// Error payload mirrored by `apps/web/src/protocol/index.ts`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    /// Stable machine code, e.g. `REVISION_CONFLICT`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Whether the client may retry.
    pub retryable: bool,
}

impl ProtocolError {
    /// Map a control error to its structured wire code.
    pub fn from_control_error(error: ControlError) -> Self {
        let (code, retryable) = match error {
            ControlError::RevisionConflict => ("REVISION_CONFLICT", true),
            ControlError::SourceForbidden => ("SOURCE_FORBIDDEN", false),
            ControlError::StaleEpoch => ("STALE_EPOCH", true),
            ControlError::Unauthorized => ("UNAUTHORIZED", false),
            ControlError::UnarmedUnmuteForbidden => ("UNMUTE_FORBIDDEN", false),
            ControlError::RateLimited => ("RATE_LIMITED", true),
            ControlError::TlsIdentityInvalid => ("TLS_IDENTITY_INVALID", false),
            ControlError::Internal => ("INTERNAL", true),
        };
        Self {
            code: code.to_string(),
            message: error.to_string(),
            retryable,
        }
    }
}

/// A complete server message ready for envelope serialization.
#[derive(Clone, PartialEq, Debug)]
pub enum ServerMessage {
    /// Acknowledgement of an accepted mix patch.
    MixAck(MixAck),
    /// Notification that a mix revision was installed by the DSP.
    MixApplied(MixApplied),
    /// Host confirmation that a listener is armed.
    ListenArmed(ListenArmed),
    /// Host SDP answer to a listener's RTC offer.
    RtcAnswer(RtcAnswer),
    /// Host trickle ICE candidate (`candidate: None` = end-of-candidates).
    RtcCandidate(RtcCandidate),
    /// Structured error response.
    ProtocolError(ProtocolError),
}

impl ServerMessage {
    /// The envelope `type` discriminator (matching ARCHITECTURE §12).
    pub fn kind(&self) -> &'static str {
        match self {
            ServerMessage::MixAck(_) => "mix.ack",
            ServerMessage::MixApplied(_) => "mix.applied",
            ServerMessage::ListenArmed(_) => "listen.armed",
            ServerMessage::RtcAnswer(_) => "rtc.answer",
            ServerMessage::RtcCandidate(_) => "rtc.candidate",
            ServerMessage::ProtocolError(_) => "error",
        }
    }

    /// The session this message is scoped to, when the wire payload carries one.
    ///
    /// RTC signaling replies carry no session field in their payload;
    /// [`Self::to_envelope_json_with_session`] attaches the authenticated session to the envelope
    /// instead.
    pub fn session_epoch(&self) -> Option<SessionEpoch> {
        match self {
            ServerMessage::MixApplied(m) => Some(m.context.session_epoch),
            ServerMessage::ListenArmed(m) => Some(m.session_epoch),
            ServerMessage::MixAck(_)
            | ServerMessage::RtcAnswer(_)
            | ServerMessage::RtcCandidate(_)
            | ServerMessage::ProtocolError(_) => None,
        }
    }

    fn payload(&self) -> serde_json::Value {
        match self {
            ServerMessage::MixAck(m) => serde_json::to_value(m),
            ServerMessage::MixApplied(m) => serde_json::to_value(m),
            ServerMessage::ListenArmed(m) => serde_json::to_value(m),
            ServerMessage::RtcAnswer(m) => serde_json::to_value(m),
            ServerMessage::RtcCandidate(m) => serde_json::to_value(m),
            ServerMessage::ProtocolError(m) => serde_json::to_value(m),
        }
        .expect("server message payload serializes")
    }

    /// Serialize into a complete `EnvelopeV1` JSON string.
    pub fn to_envelope_json(&self, host_epoch: HostEpoch, request_id: RequestId) -> String {
        self.to_envelope_json_with_session(host_epoch, request_id, self.session_epoch())
    }

    /// Serialize into a complete `EnvelopeV1` with an explicit envelope `sessionEpoch`.
    ///
    /// Used for RTC signaling replies, whose payload carries no session field but which must be
    /// scoped to the authenticated listener on the wire (matching the browser protocol).
    pub fn to_envelope_json_with_session(
        &self,
        host_epoch: HostEpoch,
        request_id: RequestId,
        session_epoch: Option<SessionEpoch>,
    ) -> String {
        let envelope = EnvelopeV1 {
            v: 1,
            kind: self.kind().to_string(),
            request_id,
            host_epoch,
            session_epoch,
            payload: self.payload(),
        };
        serde_json::to_string(&envelope).expect("server envelope serializes")
    }
}

impl From<ControlError> for ServerMessage {
    fn from(error: ControlError) -> Self {
        ServerMessage::ProtocolError(ProtocolError::from_control_error(error))
    }
}
