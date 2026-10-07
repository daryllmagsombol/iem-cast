//! WebSocket control session.
//!
//! One socket per authenticated musician. Authorization is inherited from the HTTP upgrade, which
//! already checked the origin and cookie. `mix.patch`, `listen.arm`, and `listen.disarm` envelopes
//! are dispatched to the actor; the socket holds at most one patch in flight.

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};

use crate::contract::{ControlError, EnvelopeV1};
use crate::ids::SessionEpoch;

use super::http::SharedServer;
use super::protocol::ServerMessage;

/// Maximum inbound control frame accepted from a musician.
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;

/// Drain one websocket control session until close or error.
pub async fn serve_socket(mut socket: WebSocket, state: SharedServer, session: SessionEpoch) {
    while let Some(Ok(message)) = socket.recv().await {
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
    }
    // The socket ended: disarm the authenticated session so audio does not continue silently.
    let mut guard = lock(&state);
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

fn dispatch(state: &SharedServer, session: SessionEpoch, text: &str) -> Option<String> {
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
        _ => {
            let message = ServerMessage::from(ControlError::Internal);
            Some(message.to_envelope_json(host_epoch, envelope.request_id))
        }
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
