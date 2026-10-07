//! HTTP/WSS server boundary and authorization.
//!
//! Authentication is enforced here, at the server boundary: a request is only trusted after its
//! `Secure`/`HttpOnly`/`SameSite=Strict` cookie maps to a live session, and any envelope
//! `sessionEpoch` must equal that authenticated session. A caller-supplied session is never
//! trusted.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::contract::{AssetProvider, Clock, ControlError, Entropy, EnvelopeV1, MixPatch};
use crate::control::auth::{
    set_cookie_value, session_cookie_value, OriginPolicy, OsEntropy, SessionStore, SESSION_TTL,
};
use crate::control::ControlActor;
use crate::ids::{HostEpoch, RequestId, SessionEpoch, SourceId};
use crate::transport::{MediaHub, SelectedInterface};

/// Inclusive POC cap on concurrently active receivers.
pub const POC_ACTIVE_RECEIVER_CAP: usize = 2;

use super::protocol::ServerMessage;

/// The `/api/v1/pair` request body.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    /// The single-use fragment token extracted by the musician page.
    pub token: String,
}

/// The `/api/v1/pair` success body.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResponse {
    /// The session epoch bound to the newly issued cookie.
    pub session_epoch: SessionEpoch,
}

/// The composed host security + control boundary.
pub struct HostServer {
    /// The authoritative control actor.
    pub control: ControlActor,
    /// Cookie/session store.
    pub sessions: SessionStore,
    /// Exact-match origin allowlist.
    pub origin: OriginPolicy,
    /// Shared media hub the WSS signaling path reaches (never audio; only per-listener sessions).
    pub hub: Arc<Mutex<MediaHub>>,
    assets: Arc<dyn AssetProvider>,
}

impl HostServer {
    /// Compose a server over a control actor, injected clock/entropy, origin policy, and assets.
    pub fn new(
        control: ControlActor,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        origin: OriginPolicy,
        assets: Arc<dyn AssetProvider>,
    ) -> Self {
        // The shared boundary tests construct a server without a media path; the hub is bound to a
        // loopback interface so it stays inert until a listener slot is opened over the WSS path.
        let hub = MediaHub::new(SelectedInterface {
            name: "loopback".to_string(),
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            prefix: 8,
        });
        Self::with_media(control, clock, entropy, origin, assets, Arc::new(Mutex::new(hub)))
    }

    /// Compose a server that routes WS signaling into an existing shared media hub.
    ///
    /// The hub is owned by [`crate::host::HostSession`] and shared here; [`HostServer::new`] wraps
    /// an inert loopback hub for tests and callers without a media path.
    pub fn with_media(
        control: ControlActor,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        origin: OriginPolicy,
        assets: Arc<dyn AssetProvider>,
        hub: Arc<Mutex<MediaHub>>,
    ) -> Self {
        Self {
            control,
            sessions: SessionStore::new(clock, entropy, SESSION_TTL),
            origin,
            hub,
            assets,
        }
    }

    /// The shared media hub, for wiring the UDP pump and the encoded-frame sink.
    pub fn media_hub(&self) -> Arc<Mutex<MediaHub>> {
        Arc::clone(&self.hub)
    }

    /// The global host identity.
    pub fn host_epoch(&self) -> HostEpoch {
        self.control.host_epoch()
    }

    /// A convenience server with an OS entropy source.
    pub fn with_os_entropy(
        control: ControlActor,
        clock: Arc<dyn Clock>,
        origin: OriginPolicy,
        assets: Arc<dyn AssetProvider>,
    ) -> Self {
        Self::new(control, clock, Arc::new(OsEntropy), origin, assets)
    }

    /// Resolve an authenticated session from a cookie value.
    pub fn authorize(&mut self, cookie: Option<&str>) -> Result<SessionEpoch, ControlError> {
        let token = cookie.ok_or(ControlError::Unauthorized)?;
        self.sessions
            .authenticate(token)
            .ok_or(ControlError::Unauthorized)
    }

    /// Authorize from a raw `Cookie` request header.
    pub fn authorize_header(&mut self, headers: &HeaderMap) -> Result<SessionEpoch, ControlError> {
        let value = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(session_cookie_value);
        self.authorize(value.as_deref())
    }

    /// Whether the presented origin is byte-for-byte allowed.
    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.origin.is_allowed(origin)
    }

    /// Fetch an embedded asset (diagnostics/tests).
    pub fn asset(&self, path: &str) -> Option<&'static [u8]> {
        self.assets.get(path)
    }

    /// Serve the static musician join page (never an admin page).
    pub fn join_page(&self) -> Response {
        match self.assets.get("/join") {
            Some(bytes) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                bytes,
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Exchange a single-use pairing token for a session cookie.
    pub fn handle_pair(&mut self, body: &[u8]) -> Result<(String, String), ControlError> {
        let request: PairRequest =
            serde_json::from_slice(body).map_err(|_| ControlError::Unauthorized)?;
        self.control.exchange_pairing_credential(&request.token)?;
        // POC cap: a new receiver replaces the oldest, whose session is revoked and disarmed.
        if self.sessions.active_count() >= POC_ACTIVE_RECEIVER_CAP {
            if let Some(oldest) = self.sessions.evict_oldest() {
                let generation = self.control.current_generation(oldest);
                let _ = self.control.disarm(
                    oldest,
                    crate::contract::ListenDisarm {
                        session_epoch: oldest,
                        safety_generation: generation,
                    },
                );
            }
        }
        let (token, session) = self.sessions.issue();
        let cookie = set_cookie_value(&token, SESSION_TTL);
        let payload = serde_json::to_string(&PairResponse {
            session_epoch: session,
        })
        .unwrap_or_else(|_| "{}".to_string());
        Ok((cookie, payload))
    }

    fn require_same_session(
        &self,
        authenticated: SessionEpoch,
        envelope_session: Option<SessionEpoch>,
    ) -> Result<(), ControlError> {
        match envelope_session {
            Some(s) if s == authenticated => Ok(()),
            Some(_) => Err(ControlError::Unauthorized),
            None => Ok(()),
        }
    }

    /// Handle one `mix.patch` envelope from an authenticated cookie.
    ///
    /// Returns the serialized server envelope and the HTTP status to send.
    pub fn handle_mix(
        &mut self,
        cookie: Option<&str>,
        body: &[u8],
    ) -> Result<(u16, String), ControlError> {
        let authenticated = self.authorize(cookie)?;
        let envelope: EnvelopeV1<serde_json::Value> =
            serde_json::from_slice(body).map_err(|_| ControlError::Internal)?;
        if envelope.kind != "mix.patch" {
            return Err(ControlError::Internal);
        }
        if envelope.host_epoch != self.control.host_epoch() {
            return Err(ControlError::StaleEpoch);
        }
        self.require_same_session(authenticated, envelope.session_epoch)?;

        // Idempotent replay of a bounded recent request.
        let request_id = envelope.request_id.clone();
        if let Some(ack) = self.control.cached_ack(authenticated, request_id.as_str()) {
            let message = ServerMessage::MixAck(ack);
            return Ok((200, message.to_envelope_json(self.host_epoch(), request_id)));
        }

        let patch: MixPatch =
            serde_json::from_value(envelope.payload).map_err(|_| ControlError::Internal)?;
        let ack = self.control.apply_patch(authenticated, patch)?;
        self.control
            .remember_ack(authenticated, request_id.as_str(), ack);
        let message = ServerMessage::MixAck(ack);
        Ok((200, message.to_envelope_json(self.host_epoch(), request_id)))
    }

    /// Handle one `listen.arm` envelope.
    pub fn handle_arm(
        &mut self,
        cookie: Option<&str>,
        body: &[u8],
    ) -> Result<(u16, String), ControlError> {
        let authenticated = self.authorize(cookie)?;
        let envelope: EnvelopeV1<serde_json::Value> =
            serde_json::from_slice(body).map_err(|_| ControlError::Internal)?;
        if envelope.kind != "listen.arm" {
            return Err(ControlError::Internal);
        }
        self.require_same_session(authenticated, envelope.session_epoch)?;
        let arm: crate::contract::ListenArm =
            serde_json::from_value(envelope.payload).map_err(|_| ControlError::Internal)?;
        self.control.arm(authenticated, arm)?;
        Ok((200, "{}".to_string()))
    }

    /// Handle one `listen.disarm` envelope.
    pub fn handle_disarm(
        &mut self,
        cookie: Option<&str>,
        body: &[u8],
    ) -> Result<(u16, String), ControlError> {
        let authenticated = self.authorize(cookie)?;
        let envelope: EnvelopeV1<serde_json::Value> =
            serde_json::from_slice(body).map_err(|_| ControlError::Internal)?;
        if envelope.kind != "listen.disarm" {
            return Err(ControlError::Internal);
        }
        self.require_same_session(authenticated, envelope.session_epoch)?;
        let disarm: crate::contract::ListenDisarm =
            serde_json::from_value(envelope.payload).map_err(|_| ControlError::Internal)?;
        self.control.disarm(authenticated, disarm)?;
        Ok((200, "{}".to_string()))
    }

    /// The accepted gain for an authenticated cookie's session, used by the operator/UI.
    pub fn accepted_gain_for_cookie(
        &mut self,
        cookie: &str,
        source: SourceId,
    ) -> Option<f32> {
        let session = self.sessions.authenticate(cookie)?;
        self.control.accepted_gain(session, source)
    }

    /// Build the production axum router with shared state.
    pub fn router(self) -> Router {
        let state = Arc::new(Mutex::new(self));
        Router::new()
            .route("/join", get(join_handler))
            .route("/api/v1/pair", post(pair_handler))
            .route("/api/v1/ws", get(ws_handler))
            .with_state(state)
    }
}

/// Shared axum state.
pub type SharedServer = Arc<Mutex<HostServer>>;

fn error_response(error: ControlError) -> Response {
    let status = match error {
        ControlError::Unauthorized | ControlError::UnarmedUnmuteForbidden => {
            StatusCode::FORBIDDEN
        }
        ControlError::TlsIdentityInvalid => StatusCode::INTERNAL_SERVER_ERROR,
        ControlError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ControlError::RevisionConflict | ControlError::StaleEpoch => StatusCode::CONFLICT,
        ControlError::SourceForbidden => StatusCode::FORBIDDEN,
        ControlError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = ServerMessage::from(error);
    (status, message.to_envelope_json(HostEpoch(uuid::Uuid::nil()), RequestId(String::new())))
        .into_response()
}

/// Exact allowed-origins gate, evaluated before any WebSocket upgrade.
///
/// Returns `101` when the origin is allowed and `403` otherwise.
pub fn origin_gate(server: &HostServer, origin: &str) -> u16 {
    if server.origin_allowed(origin) {
        101
    } else {
        403
    }
}

async fn join_handler(State(state): State<SharedServer>) -> Response {
    let guard = state.lock().expect("server state");
    guard.join_page()
}

async fn pair_handler(State(state): State<SharedServer>, body: Bytes) -> Response {
    let mut guard = state.lock().expect("server state");
    match guard.handle_pair(&body) {
        Ok((cookie, payload)) => (
            StatusCode::OK,
            [
                (axum::http::header::SET_COOKIE, cookie),
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/json".to_string(),
                ),
            ],
            payload,
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}

async fn ws_handler(
    State(state): State<SharedServer>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Origin is checked BEFORE any upgrade work.
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    {
        let guard = state.lock().expect("server state");
        if !guard.origin_allowed(origin) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    // Authenticate before upgrading as well; the socket inherits this session.
    let session = {
        let mut guard = state.lock().expect("server state");
        match guard.authorize_header(&headers) {
            Ok(session) => session,
            Err(error) => return error_response(error),
        }
    };
    upgrade.on_upgrade(move |socket| super::ws::serve_socket(socket, state, session))
}
