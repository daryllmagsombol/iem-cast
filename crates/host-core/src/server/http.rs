//! HTTP/WSS server boundary and authorization.
//!
//! Authentication is enforced here, at the server boundary: a request is only trusted after its
//! `Secure`/`HttpOnly`/`SameSite=Strict` cookie maps to a live session, and any envelope
//! `sessionEpoch` must equal that authenticated session. A caller-supplied session is never
//! trusted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::contract::{
    AssetProvider, AudioEvent, CatalogSnapshot, Clock, ControlError, Entropy, EnvelopeV1,
    ListenerPhase, MixPatch, SessionContext, SessionSnapshot,
};
use crate::control::auth::{
    set_cookie_value, session_cookie_value, OriginPolicy, OsEntropy, SessionStore, SESSION_TTL,
};
use crate::control::ControlActor;
use crate::ids::{CatalogRevision, HostEpoch, RequestId, SessionEpoch, SourceId};
use crate::transport::{MediaHub, SelectedInterface};

/// Inclusive POC cap on concurrently active receivers.
pub const POC_ACTIVE_RECEIVER_CAP: usize = 2;

/// Bound on a single session's outbound push queue.
///
/// A slow or dead socket causes the oldest queued pushes to be dropped rather than the control
/// plane stalling: the sender uses `try_send` and never blocks the caller holding the server lock.
pub const OUTBOUND_QUEUE_CAPACITY: usize = 256;

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
    /// The versioned source catalog, sent to a listener as an initial `catalog.snapshot`.
    catalog: CatalogSnapshot,
    /// Per-authenticated-session outbound push channels (unsolicited server messages).
    outbound: HashMap<SessionEpoch, mpsc::Sender<String>>,
    /// Bounded receiver for [`AudioEvent`]s published by the DSP bridge.
    ///
    /// The bridge owns the matching non-blocking `SyncSender`; the server drains this on the
    /// socket loop so a stalled audio path can never block on the server lock.
    audio_events: Option<std::sync::mpsc::Receiver<AudioEvent>>,
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
    ///
    /// The source `catalog` is empty here for backward compatibility with callers (including the
    /// server-boundary tests) that predate catalog snapshots; use
    /// [`with_media_and_catalog`](Self::with_media_and_catalog) to supply the real catalog.
    pub fn with_media(
        control: ControlActor,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        origin: OriginPolicy,
        assets: Arc<dyn AssetProvider>,
        hub: Arc<Mutex<MediaHub>>,
    ) -> Self {
        Self::with_media_and_catalog(
            control,
            clock,
            entropy,
            origin,
            assets,
            hub,
            CatalogSnapshot {
                catalog_revision: CatalogRevision(0),
                sources: Vec::new(),
            },
        )
    }

    /// Compose a server over an explicit source catalog.
    ///
    /// This is the production constructor used by [`crate::host::HostSession`]; the catalog is sent
    /// to each connecting listener as its initial `catalog.snapshot`.
    pub fn with_media_and_catalog(
        control: ControlActor,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        origin: OriginPolicy,
        assets: Arc<dyn AssetProvider>,
        hub: Arc<Mutex<MediaHub>>,
        catalog: CatalogSnapshot,
    ) -> Self {
        Self {
            control,
            sessions: SessionStore::new(clock, entropy, SESSION_TTL),
            origin,
            hub,
            assets,
            catalog,
            outbound: HashMap::new(),
            audio_events: None,
        }
    }

    /// Install the receiver for [`AudioEvent`]s published by the DSP bridge.
    ///
    /// The matching non-blocking sender is held by the bridge, which calls `try_send`, so the
    /// audio path never blocks on the server lock.
    pub fn set_audio_events(&mut self, receiver: std::sync::mpsc::Receiver<AudioEvent>) {
        self.audio_events = Some(receiver);
    }

    /// Drain and apply every queued audio event. Called by the socket loop with a short cadence;
    /// returns how many events were applied.
    pub fn drain_audio_events(&mut self) -> usize {
        let mut applied = 0;
        loop {
            let event = match self.audio_events.as_ref() {
                Some(receiver) => receiver.try_recv(),
                None => return applied,
            };
            match event {
                Ok(event) => {
                    self.publish_audio_event(event);
                    applied += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return applied,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return applied,
            }
        }
    }

    /// The shared media hub, for wiring the UDP pump and the encoded-frame sink.
    pub fn media_hub(&self) -> Arc<Mutex<MediaHub>> {
        Arc::clone(&self.hub)
    }

    /// The source catalog listeners patch against.
    pub fn catalog(&self) -> &CatalogSnapshot {
        &self.catalog
    }

    /// Register the outbound push channel for an authenticated session.
    ///
    /// Returns the receiver the socket loop selects on and a clone of the sender, which the socket
    /// loop keeps solely to prove ownership when it later unregisters. A previous registration for
    /// the same session (a reconnect) is replaced; its sender is dropped, ending the stale socket.
    pub fn register_outbound(
        &mut self,
        session: SessionEpoch,
    ) -> (mpsc::Receiver<String>, mpsc::Sender<String>) {
        let (sender, receiver) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        self.outbound.insert(session, sender.clone());
        (receiver, sender)
    }

    /// Unregister a session's outbound channel once its socket ends.
    ///
    /// Only removes the entry when `sender` is still the registered channel for `session`, so a
    /// newer reconnect cannot have its channel torn down by the old socket's cleanup.
    pub fn unregister_outbound(
        &mut self,
        session: SessionEpoch,
        sender: &mpsc::Sender<String>,
    ) {
        if let Some(existing) = self.outbound.get(&session) {
            if existing.same_channel(sender) {
                self.outbound.remove(&session);
            }
        }
    }

    /// Queue an envelope to a session's socket. Non-blocking: a full queue drops the message
    /// (oldest-effort push) rather than stalling the control plane, and a closed receiver is
    /// simply ignored.
    pub fn push_outbound(&mut self, session: SessionEpoch, envelope: String) {
        if let Some(sender) = self.outbound.get(&session) {
            let _ = sender.try_send(envelope);
        }
    }

    /// The initial `session.snapshot` for a connecting session.
    ///
    /// The live phase and accepted/applied mixes come from the actor; requested mix is not tracked
    /// separately from accepted at this boundary, so it is `None`.
    pub fn session_snapshot(&self, session: SessionEpoch) -> SessionSnapshot {
        let accepted = self.control.accepted_snapshot(session);
        let accepted_mix = accepted;
        SessionSnapshot {
            session_epoch: session,
            context: SessionContext {
                session_epoch: session,
                audio_epoch: self.control.audio_epoch(),
                safety_generation: self.control.current_generation(session),
            },
            phase: if accepted_mix.is_some() {
                ListenerPhase::ReadyMuted
            } else {
                ListenerPhase::Paired
            },
            catalog_revision: self.catalog.catalog_revision,
            requested_mix: None,
            accepted_mix,
            applied_mix: None,
        }
    }

    /// Register a newly connected session's outbound channel and enqueue its initial snapshots.
    ///
    /// This is the exact connect sequence [`crate::server::ws::serve_socket`] performs immediately
    /// after the authenticated upgrade, factored out so it is testable without a live socket.
    pub fn connect_session(
        &mut self,
        session: SessionEpoch,
    ) -> (mpsc::Receiver<String>, mpsc::Sender<String>) {
        let (receiver, sender) = self.register_outbound(session);
        for envelope in self.initial_snapshots(session) {
            self.push_outbound(session, envelope);
        }
        (receiver, sender)
    }

    /// Build the initial snapshots pushed immediately after a socket upgrade.
    pub fn initial_snapshots(&self, session: SessionEpoch) -> Vec<String> {
        let host_epoch = self.control.host_epoch();
        let session_snapshot = ServerMessage::SessionSnapshot(self.session_snapshot(session))
            .to_envelope_json(host_epoch, RequestId(String::new()));
        let catalog_snapshot = ServerMessage::CatalogSnapshot(self.catalog.clone())
            .to_envelope_json(host_epoch, RequestId(String::new()));
        vec![session_snapshot, catalog_snapshot]
    }

    /// Feed an audio event through the actor and push any resulting server message to the owning
    /// session's socket. This is the only path by which `listen.armed`/`mix.applied` reach a phone.
    ///
    /// A message is routed to exactly the session named by the actor's validated output; an event
    /// whose session has no registered socket is dropped.
    pub fn publish_audio_event(&mut self, ev: AudioEvent) {
        let message = match self.control.on_audio_event(ev) {
            Some(message) => message,
            None => return,
        };
        let session = match message.session_epoch() {
            Some(session) => session,
            None => return,
        };
        let host_epoch = self.control.host_epoch();
        let envelope = message.to_envelope_json(host_epoch, RequestId(String::new()));
        self.push_outbound(session, envelope);
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
        Self::router_from_shared(self.into_shared())
    }

    /// Move this server into shared state so the running router and an external handle (e.g. the
    /// desktop's pairing-credential command) can both reach the same authoritative actor.
    pub fn into_shared(self) -> SharedServer {
        Arc::new(Mutex::new(self))
    }

    /// Build the production axum router over an existing shared state.
    ///
    /// This lets a composition keep a handle to the same server the router serves, so operator
    /// actions (minting a pairing credential) are applied by the one authoritative actor.
    pub fn router_from_shared(state: SharedServer) -> Router {
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
