//! WebSocket control session.
//!
//! One socket per authenticated musician. Authorization is inherited from the HTTP upgrade, which
//! already checked the origin and cookie. `mix.patch`, `listen.arm`, and `listen.disarm` envelopes
//! are dispatched to the actor; the socket holds at most one patch in flight.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};

use crate::audio::engine::MAX_SESSIONS;
use crate::contract::{ControlError, EnvelopeV1, RtcAnswer, RtcCandidate, RtcOffer};
use crate::ids::SessionEpoch;

use super::http::SharedServer;
use super::protocol::ServerMessage;

/// Maximum inbound control frame accepted from a musician.
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;

/// How often an idle socket re-checks the audio-event queue.
const AUDIO_EVENT_POLL: Duration = Duration::from_millis(25);

/// One iteration of the socket loop, resolved by whichever of recv/push/tick is ready.
enum Step {
    Inbound(Message),
    Push(String),
    Tick,
}

/// Drain one websocket control session until close or error.
///
/// The loop serves both directions: inbound control frames are dispatched to the actor, while
/// unsolicited server messages (initial snapshots, `listen.armed`, `mix.applied`) are forwarded
/// from this session's bounded outbound channel. Audio events published by the DSP bridge are
/// applied between frames, so a phone that never sends anything still receives its pushes.
pub async fn serve_socket(mut socket: WebSocket, state: SharedServer, session: SessionEpoch) {
    // Register the push channel and queue the initial catalog/session snapshots before serving.
    let (mut outbound_rx, outbound_tx) = lock(&state).connect_session(session);

    let mut ticker = tokio::time::interval(AUDIO_EVENT_POLL);
    // The first tick fires immediately; consume it so the loop does not spin.
    ticker.tick().await;

    loop {
        let step = tokio::select! {
            maybe = socket.recv() => match maybe {
                Some(Ok(message)) => Step::Inbound(message),
                _ => break,
            },
            maybe = outbound_rx.recv() => match maybe {
                Some(envelope) => Step::Push(envelope),
                None => break,
            },
            _ = ticker.tick() => Step::Tick,
        };

        match step {
            Step::Inbound(message) => {
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => text,
                        Err(_) => continue,
                    },
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => continue,
                };
                if text.len() > MAX_CONTROL_FRAME_BYTES {
                    continue;
                }
                if let Some(reply) = dispatch(&state, session, &text) {
                    if socket.send(Message::Text(reply.into())).await.is_err() {
                        break;
                    }
                }
                // A dispatch may have queued an audio event (e.g. an accepted patch); apply it now
                // so `mix.applied`/`listen.armed` are pushed on the next loop turn.
                lock(&state).drain_audio_events();
            }
            Step::Push(envelope) => {
                if socket.send(Message::Text(envelope.into())).await.is_err() {
                    break;
                }
            }
            Step::Tick => {
                lock(&state).drain_audio_events();
            }
        }
    }

    // The socket ended: stop routing pushes to it and disarm the session so audio does not
    // continue silently.
    let mut guard = lock(&state);
    guard.unregister_outbound(session, &outbound_tx);
    let generation = guard.control.current_generation(session);
    let _ = guard.control.disarm(
        session,
        crate::contract::ListenDisarm {
            session_epoch: session,
            safety_generation: generation,
        },
    );
}

fn lock(state: &SharedServer) -> std::sync::MutexGuard<'_, super::http::HostServer> {
    state.lock().expect("server state")
}

pub(crate) fn dispatch(
    state: &SharedServer,
    session: SessionEpoch,
    text: &str,
) -> Option<String> {
    let envelope: EnvelopeV1<serde_json::Value> = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(_) => {
            // No trustworthy epoch/request id is available for an unparseable frame; drop it.
            return None;
        }
    };
    let host_epoch = {
        let guard = lock(state);
        guard.control.host_epoch()
    };
    if envelope.host_epoch != host_epoch {
        let message = ServerMessage::from(ControlError::StaleEpoch);
        return Some(message.to_envelope_json(host_epoch, envelope.request_id));
    }
    // The socket is already authenticated as `session`; envelope session must match.
    if let Some(env_session) = envelope.session_epoch {
        if env_session != session {
            let message = ServerMessage::from(ControlError::Unauthorized);
            return Some(message.to_envelope_json(host_epoch, envelope.request_id));
        }
    }

    match envelope.kind.as_str() {
        "mix.patch" => handle_patch(state, session, host_epoch, envelope),
        "listen.arm" => handle_arm(state, session, host_epoch, envelope),
        "listen.disarm" => handle_disarm(state, session, host_epoch, envelope),
        "rtc.offer" => handle_offer(state, session, host_epoch, envelope),
        "rtc.candidate" => handle_candidate(state, session, host_epoch, envelope),
        _ => {
            let message = ServerMessage::from(ControlError::Internal);
            Some(message.to_envelope_json(host_epoch, envelope.request_id))
        }
    }
}

/// Resolve the media slot bound to `session`, opening a free slot if none exists.
///
/// The WSS socket is already authenticated as `session`; a slot bound to a different session is
/// never reused, so one listener can never signal into another's media path.
fn ensure_slot(state: &SharedServer, session: SessionEpoch) -> Result<usize, ControlError> {
    let guard = lock(state);
    let mut hub = guard.hub.lock().expect("media hub");
    for slot in 0..MAX_SESSIONS {
        if hub.session_at(slot) == Some(session) {
            return Ok(slot);
        }
    }
    for slot in 0..MAX_SESSIONS {
        if hub.session_at(slot).is_none() {
            hub.open_slot(slot, session)
                .map_err(|_| ControlError::Internal)?;
            return Ok(slot);
        }
    }
    Err(ControlError::Internal)
}

fn handle_offer(
    state: &SharedServer,
    session: SessionEpoch,
    host_epoch: crate::ids::HostEpoch,
    envelope: EnvelopeV1<serde_json::Value>,
) -> Option<String> {
    let request_id = envelope.request_id;
    let offer: RtcOffer = match serde_json::from_value(envelope.payload) {
        Ok(offer) => offer,
        Err(_) => {
            return Some(
                ServerMessage::from(ControlError::Internal)
                    .to_envelope_json(host_epoch, request_id),
            )
        }
    };
    let slot = match ensure_slot(state, session) {
        Ok(slot) => slot,
        Err(error) => {
            return Some(ServerMessage::from(error).to_envelope_json(host_epoch, request_id))
        }
    };
    let answer = {
        let guard = lock(state);
        let mut hub = guard.hub.lock().expect("media hub");
        hub.handle_offer(slot, &offer.sdp)
    };
    match answer {
        Ok(sdp) => Some(
            ServerMessage::RtcAnswer(RtcAnswer { sdp }).to_envelope_json_with_session(
                host_epoch,
                request_id,
                Some(session),
            ),
        ),
        Err(_) => Some(
            ServerMessage::from(ControlError::Internal).to_envelope_json(host_epoch, request_id),
        ),
    }
}

fn handle_candidate(
    state: &SharedServer,
    session: SessionEpoch,
    host_epoch: crate::ids::HostEpoch,
    envelope: EnvelopeV1<serde_json::Value>,
) -> Option<String> {
    let request_id = envelope.request_id;
    let candidate: RtcCandidate = match serde_json::from_value(envelope.payload) {
        Ok(candidate) => candidate,
        Err(_) => {
            return Some(
                ServerMessage::from(ControlError::Internal)
                    .to_envelope_json(host_epoch, request_id),
            )
        }
    };
    let slot = match ensure_slot(state, session) {
        Ok(slot) => slot,
        Err(error) => {
            return Some(ServerMessage::from(error).to_envelope_json(host_epoch, request_id))
        }
    };
    let result = {
        let guard = lock(state);
        let mut hub = guard.hub.lock().expect("media hub");
        hub.add_candidate(slot, candidate.candidate.as_deref())
    };
    match result {
        // The host is a non-trickle answerer: host candidates are already in the answer SDP, so the
        // acknowledgement is an explicit end-of-candidates for the listener's own candidate.
        Ok(()) => Some(
            ServerMessage::RtcCandidate(RtcCandidate {
                candidate: None,
                sdp_mid: candidate.sdp_mid,
                sdp_m_line_index: candidate.sdp_m_line_index,
            })
            .to_envelope_json_with_session(host_epoch, request_id, Some(session)),
        ),
        Err(_) => Some(
            ServerMessage::from(ControlError::Internal).to_envelope_json(host_epoch, request_id),
        ),
    }
}

fn handle_patch(
    state: &SharedServer,
    session: SessionEpoch,
    host_epoch: crate::ids::HostEpoch,
    envelope: EnvelopeV1<serde_json::Value>,
) -> Option<String> {
    let request_id = envelope.request_id;
    let mut guard = lock(state);
    if let Some(ack) = guard
        .control
        .cached_ack(session, request_id.as_str())
    {
        return Some(ServerMessage::MixAck(ack).to_envelope_json(host_epoch, request_id));
    }
    let patch: crate::contract::MixPatch = match serde_json::from_value(envelope.payload) {
        Ok(patch) => patch,
        Err(_) => {
            return Some(
                ServerMessage::from(ControlError::Internal)
                    .to_envelope_json(host_epoch, request_id),
            )
        }
    };
    match guard.control.apply_patch(session, patch) {
        Ok(ack) => {
            guard
                .control
                .remember_ack(session, request_id.as_str(), ack);
            Some(ServerMessage::MixAck(ack).to_envelope_json(host_epoch, request_id))
        }
        Err(error) => Some(ServerMessage::from(error).to_envelope_json(host_epoch, request_id)),
    }
}

fn handle_arm(
    state: &SharedServer,
    session: SessionEpoch,
    host_epoch: crate::ids::HostEpoch,
    envelope: EnvelopeV1<serde_json::Value>,
) -> Option<String> {
    let request_id = envelope.request_id;
    let arm: crate::contract::ListenArm = match serde_json::from_value(envelope.payload) {
        Ok(arm) => arm,
        Err(_) => {
            return Some(
                ServerMessage::from(ControlError::Internal)
                    .to_envelope_json(host_epoch, request_id),
            )
        }
    };
    let mut guard = lock(state);
    match guard.control.arm(session, arm) {
        Ok(()) => None, // `listen.armed` is emitted only when the DSP confirms the tuple.
        Err(error) => Some(ServerMessage::from(error).to_envelope_json(host_epoch, request_id)),
    }
}

fn handle_disarm(
    state: &SharedServer,
    session: SessionEpoch,
    host_epoch: crate::ids::HostEpoch,
    envelope: EnvelopeV1<serde_json::Value>,
) -> Option<String> {
    let request_id = envelope.request_id;
    let disarm: crate::contract::ListenDisarm = match serde_json::from_value(envelope.payload) {
        Ok(disarm) => disarm,
        Err(_) => {
            return Some(
                ServerMessage::from(ControlError::Internal)
                    .to_envelope_json(host_epoch, request_id),
            )
        }
    };
    let mut guard = lock(state);
    match guard.control.disarm(session, disarm) {
        Ok(()) => None,
        Err(error) => Some(ServerMessage::from(error).to_envelope_json(host_epoch, request_id)),
    }
}

/// Shared state type re-export alias used by the actor-driven emit path.
pub type WsState = Arc<Mutex<super::http::HostServer>>;
