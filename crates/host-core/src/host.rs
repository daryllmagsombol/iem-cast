//! Host process composition: capture + DSP + media transport, driven end to end.
//!
//! [`HostSession`] is the missing composition the POC lacked: it starts the capture/DSP
//! [`HostRuntime`] with an [`EncodedSink`] that routes each listener's encoded frames into a shared
//! [`MediaHub`], binds the UDP socket the media sessions send on, and runs a small pump thread that
//! feeds inbound datagrams into the hub, advances its timers, and emits its outbound datagrams.
//!
//! Scope and honesty: this composition owns capture and the media datagram path. It deliberately
//! does **not** own the HTTPS/WSS control server; the server shares the same `MediaHub` through
//! [`crate::server::HostServer::with_media`] so WS signaling reaches the same per-listener sessions.
//! Real device capture is only reached through [`HostSession::start_real`]; tests always inject a
//! fake backend and a loopback socket.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;

use crate::audio::engine::MAX_SESSIONS;
use crate::capture::service::{CaptureBackend, SystemCaptureBackend};
use crate::contract::{
    AssetProvider, AudioEvent, CaptureFault, CatalogSnapshot, ChannelMapEntry, Clock, ControlError,
    InterruptReason, MixSnapshot, SourceInfo, SourceRole, StartHostRequest, SystemClock,
};
use crate::control::actor::ControlActor;
use crate::control::auth::{OriginPolicy, OsEntropy};
use crate::ids::{
    AudioEpoch, CatalogRevision, ChannelMapRevision, HostEpoch, SessionEpoch, SourceId,
};
use crate::pipeline::ListenerOutput;
use crate::runtime::{EncodedSink, HostRuntime};
use crate::server::tls::TlsIdentity;
use crate::server::HostServer;
use crate::transport::{MediaHub, SelectedInterface};

/// How long the pump sleeps when no datagram is ready before re-checking timers and the stop flag.
const PUMP_IDLE: Duration = Duration::from_millis(1);
/// Maximum inbound datagram accepted by the pump (a bounded RTP packet fits well within).
const MAX_DATAGRAM_BYTES: usize = 2048;
/// Bound on the DSP→server audio-event queue; overflow drops rather than stalling audio.
const AUDIO_EVENT_QUEUE_CAPACITY: usize = 256;

/// Composition/lifecycle failure for [`HostSession`].
#[derive(Debug, thiserror::Error)]
pub enum HostFault {
    /// The capture/DSP runtime failed to start.
    #[error("capture: {0}")]
    Capture(#[from] CaptureFault),
    /// The TLS identity is missing, unreadable, or invalid.
    #[error("tls: {0}")]
    Tls(#[from] ControlError),
    /// The media socket could not be bound or configured.
    #[error("media socket: {0}")]
    Socket(#[from] std::io::Error),
}

/// Routes the runtime's encoded frames into a shared [`MediaHub`].
///
/// This is the bridge from the DSP worker thread to the WebRTC sessions. It holds the same
/// `Arc<Mutex<MediaHub>>` the server and the UDP pump hold, so a listener's frames reach its own
/// session while the control plane negotiates that session on the WS socket.
struct MediaHubSink {
    hub: Arc<Mutex<MediaHub>>,
}

impl EncodedSink for MediaHubSink {
    fn on_block(&self, outputs: &[ListenerOutput; MAX_SESSIONS]) {
        let mut hub = self.hub.lock().expect("media hub");
        hub.write_block(outputs);
    }
}

/// Non-blocking bridge from the DSP path to the server's audio-event queue.
///
/// The audio/DSP path must never block on the server lock, so this is a bounded `try_send`: a full
/// queue (a stalled consumer) drops the event rather than stalling audio.
struct ChannelAudioEventSink {
    tx: std::sync::mpsc::SyncSender<AudioEvent>,
}

impl crate::contract::AudioEventSink for ChannelAudioEventSink {
    fn try_publish(&self, ev: AudioEvent) -> bool {
        self.tx.try_send(ev).is_ok()
    }
}

/// Forwards the control actor's DSP commands into the running runtime and media hub.
///
/// A mix may arrive before the listener's media slot exists (e.g. a patch before `rtc.offer`).
/// Those snapshots are held in `pending` and flushed when the session arms, so ordering between
/// signaling and control cannot silently drop a mix.
///
/// After forwarding a command, the bridge publishes the corresponding [`AudioEvent`] to the
/// server's outbound path (when a sink is installed), which is what lets `mix.applied` and
/// `listen.armed` reach the phone. The publish is non-blocking.
struct DspBridge {
    handle: crate::runtime::RuntimeHandle,
    hub: Arc<Mutex<MediaHub>>,
    audio_epoch: AudioEpoch,
    pending: Mutex<std::collections::HashMap<SessionEpoch, MixSnapshot>>,
    events: Option<Arc<dyn crate::contract::AudioEventSink>>,
}

impl DspBridge {
    /// The media slot bound to a session, if the listener has negotiated one.
    fn slot_for(&self, session: SessionEpoch) -> Option<usize> {
        let hub = self.hub.lock().ok()?;
        (0..MAX_SESSIONS).find(|&slot| hub.session_at(slot) == Some(session))
    }

    /// Publish one event to the server's outbound path, if a sink is installed. Never blocks.
    fn publish(&self, ev: AudioEvent) {
        if let Some(sink) = &self.events {
            let _ = sink.try_publish(ev);
        }
    }
}

impl crate::contract::DspControl for DspBridge {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError> {
        let session = snapshot.context.session_epoch;
        match self.slot_for(session) {
            Some(slot) => {
                let context = snapshot.context;
                let applied_revision = snapshot.mix_revision;
                self.handle.set_mix(slot, session, snapshot);
                self.publish(AudioEvent::MixApplied {
                    applied_revision,
                    context,
                    start_sample: 0,
                    snapshot,
                });
            }
            None => {
                // No media session yet; remember it so arming can apply it.
                if let Ok(mut pending) = self.pending.lock() {
                    pending.insert(session, snapshot);
                }
            }
        }
        Ok(())
    }

    fn request_arm(&self, arm: crate::contract::ListenArm) -> Result<(), ControlError> {
        let session = arm.context.session_epoch;
        // Apply any mix that arrived before signaling opened the media slot.
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&session));
        if let (Some(slot), Some(snapshot)) = (self.slot_for(session), pending) {
            let context = snapshot.context;
            let applied_revision = snapshot.mix_revision;
            self.handle.set_mix(slot, session, snapshot);
            self.publish(AudioEvent::MixApplied {
                applied_revision,
                context,
                start_sample: 0,
                snapshot,
            });
        }
        // Open the per-session media gate with the exact context the actor assigned.
        if let Some(slot) = self.slot_for(session) {
            if let Ok(mut hub) = self.hub.lock() {
                hub.arm(slot, arm.context);
            }
        }
        // The gate is open for the exact tuple the actor assigned; confirm it.
        self.publish(AudioEvent::ArmApplied {
            context: arm.context,
            arm_nonce: arm.arm_nonce,
        });
        Ok(())
    }

    fn disarm(&self, session: SessionEpoch, new_generation: crate::ids::SafetyGeneration) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&session);
        }
        if let Some(slot) = self.slot_for(session) {
            let context = crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: self.audio_epoch,
                safety_generation: new_generation,
            };
            if let Ok(mut hub) = self.hub.lock() {
                hub.disarm(slot, context);
            }
        }
        self.publish(AudioEvent::Interrupted {
            session_epoch: session,
            reason: InterruptReason::UserDisarm,
        });
    }
}

/// A fully started local host: capture, DSP, media sessions, and the UDP pump.
pub struct HostSession {
    runtime: HostRuntime,
    hub: Arc<Mutex<MediaHub>>,
    socket: UdpSocket,
    audio_epoch: AudioEpoch,
    stopped: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
}

impl HostSession {
    /// Start a host against an injected capture backend (tests never open real hardware).
    ///
    /// The `interface` is where the media sessions advertise their host candidate and where the
    /// UDP socket binds. The TLS identity is validated here as a precondition, even though the
    /// control server is composed separately.
    pub fn start<B: CaptureBackend + ?Sized>(
        backend: &B,
        tls: TlsIdentity,
        interface: SelectedInterface,
        request: StartHostRequest,
        channel_map: Vec<ChannelMapEntry>,
    ) -> Result<Self, HostFault> {
        tls.validate_paths()?;

        // Bind the media socket FIRST so the hub advertises the real bound port. If the hub were
        // created with the caller's port (often 0), every WebRTC host candidate would point at
        // port 0 and ICE could never connect.
        let socket = UdpSocket::bind(SocketAddr::new(interface.ip, interface.port))?;
        socket.set_nonblocking(true)?;
        let bound_port = socket.local_addr()?.port();
        let mut advertised = interface;
        advertised.port = bound_port;

        let hub = Arc::new(Mutex::new(MediaHub::new(advertised)));
        let sink = MediaHubSink {
            hub: Arc::clone(&hub),
        };

        let audio_epoch = crate::capture::next_audio_epoch(AudioEpoch(uuid::Uuid::nil()));
        let capture: crate::contract::CaptureRequest = request.capture.clone();
        let runtime = HostRuntime::start(
            backend,
            capture,
            audio_epoch,
            ChannelMapRevision(0),
            channel_map,
            sink,
        )?;

        let stopped = Arc::new(AtomicBool::new(false));
        let pump_hub = Arc::clone(&hub);
        let pump_stopped = Arc::clone(&stopped);
        let pump_socket = socket.try_clone().map_err(HostFault::Socket)?;
        let pump = thread::Builder::new()
            .name("iem-media-pump".to_string())
            .spawn(move || {
                pump_loop(pump_socket, pump_hub, pump_stopped);
            })
            .map_err(HostFault::Socket)?;

        Ok(Self {
            runtime,
            hub,
            socket,
            audio_epoch,
            stopped,
            pump: Some(pump),
        })
    }

    /// Start a host against the real CoreAudio capture backend.
    pub fn start_real(
        tls: TlsIdentity,
        interface: SelectedInterface,
        request: StartHostRequest,
        channel_map: Vec<ChannelMapEntry>,
    ) -> Result<Self, HostFault> {
        Self::start(&SystemCaptureBackend, tls, interface, request, channel_map)
    }

    /// The shared media hub (for wiring the WS signaling path / the control server).
    pub fn media_hub(&self) -> Arc<Mutex<MediaHub>> {
        Arc::clone(&self.hub)
    }

    /// A [`DspControl`] bridge so the control actor's installed mixes reach the DSP worker.
    ///
    /// Without this, the actor would accept patches that never affected audio. The bridge maps a
    /// session to its media slot, forwards mixes to the runtime, and opens/closes the per-session
    /// media gate on arm/disarm.
    pub fn dsp_bridge(&self) -> Arc<dyn crate::contract::DspControl> {
        self.dsp_bridge_with_events(None)
    }

    /// A [`DspControl`] bridge that also publishes [`AudioEvent`]s to `sink`.
    ///
    /// The composition installs a bounded, non-blocking sink here so the DSP path's arm/mix/disarm
    /// confirmations reach the control server's outbound push path.
    pub fn dsp_bridge_with_events(
        &self,
        sink: Option<Arc<dyn crate::contract::AudioEventSink>>,
    ) -> Arc<dyn crate::contract::DspControl> {
        Arc::new(DspBridge {
            handle: self.runtime.handle(),
            hub: Arc::clone(&self.hub),
            audio_epoch: self.audio_epoch,
            pending: Mutex::new(std::collections::HashMap::new()),
            events: sink,
        })
    }

    /// The capture generation this session minted for its blocks.
    pub fn audio_epoch(&self) -> AudioEpoch {
        self.audio_epoch
    }

    /// A cloneable handle for driving the DSP worker (e.g. selecting the local monitor slot).
    pub fn runtime_handle(&self) -> crate::runtime::RuntimeHandle {
        self.runtime.handle()
    }

    /// The local address the media socket is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Stop capture, stop the media pump, and join cleanly. Idempotent.
    pub fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.runtime.stop();
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A running HTTPS/WSS control server composed over a [`HostSession`].
///
/// This is the missing piece that turns the capture-only [`HostSession`] into something a phone can
/// actually join: it builds the authoritative [`ControlActor`] with the session's DSP bridge,
/// composes [`HostServer::with_media`] over the **same** [`MediaHub`], and serves the router over
/// TLS on a dedicated background tokio runtime.
///
/// The server runs on its own single-threaded runtime thread so it does not depend on the caller
/// having (or being inside) a tokio runtime and can be embedded in a synchronous Tauri command.
/// [`stop`](Self::stop) performs a bounded graceful shutdown and joins that thread; dropping the
/// handle also stops it.
pub struct HostServerHandle {
    addr: SocketAddr,
    join_url: String,
    /// The one authoritative server the router serves, kept so the operator can mint fresh pairing
    /// credentials against the same actor (never a second, divergent instance).
    shared: crate::server::http::SharedServer,
    handle: axum_server::Handle<SocketAddr>,
    server_thread: Option<JoinHandle<()>>,
}

impl HostServerHandle {
    /// The bound local address (useful when binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The operator-facing join URL carrying the initial single-use pairing credential.
    ///
    /// The token in this URL expires after the pairing TTL (120 s); use the operator's
    /// pairing-credential path for a fresh token after that.
    pub fn join_url(&self) -> &str {
        &self.join_url
    }

    /// Mint a fresh single-use pairing credential from the running host's own `PairStore`.
    ///
    /// Returns `None` only if the shared server lock is poisoned; a genuinely minted credential is
    /// always returned otherwise. The token is never logged.
    pub fn issue_pairing_credential(
        &self,
    ) -> Option<crate::server::PairingCredential> {
        let mut guard = self.shared.lock().ok()?;
        Some(guard.control.issue_pairing_credential())
    }

    /// Gracefully stop receiving, then join the server thread. Idempotent.
    ///
    /// A bounded grace period avoids hanging on a wedged connection; dropping the background
    /// runtime (when the thread exits) cancels any remaining tasks.
    pub fn stop(&mut self) {
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        if let Some(thread) = self.server_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for HostServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The composition inputs for [`HostSession::start_server`], so callers do not pass a long
/// positional argument list.
pub struct ServerConfig {
    /// TLS certificate + key (used by [`HostSession::start_server`] to load the rustls config).
    pub tls: TlsIdentity,
    /// Address to bind the HTTPS listener on.
    pub bind: SocketAddr,
    /// Exact-match origin allowlist for the WS upgrade.
    pub origin: OriginPolicy,
    /// Embedded musician assets served from `/join`.
    pub assets: Arc<dyn AssetProvider>,
    /// The versioned source catalog the actor validates patches against.
    pub catalog: CatalogSnapshot,
}

impl HostSession {
    /// Start the HTTPS/WSS control server over this session. See [`HostServerHandle`].
    ///
    /// `bundle` provides the TLS identity, bind address, origin policy, assets, and catalog. Every
    /// piece needed to accept a phone is derived here: the actor gets this session's
    /// [`dsp_bridge`](Self::dsp_bridge), and the server shares this session's
    /// [`media_hub`](Self::media_hub), so WS signaling reaches the live media sessions.
    pub fn start_server(&self, bundle: ServerConfig) -> Result<HostServerHandle, HostFault> {
        let config = load_rustls_config(&bundle.tls)?;
        self.start_server_with_config(config, bundle)
    }

    /// Start the control server with an already-loaded [`RustlsConfig`].
    ///
    /// This is the testable seam: it does not require real certificate/key files on disk. The
    /// production path is [`start_server`](Self::start_server), which loads the config from the
    /// TLS identity's PEM paths.
    pub fn start_server_with_config(
        &self,
        config: RustlsConfig,
        bundle: ServerConfig,
    ) -> Result<HostServerHandle, HostFault> {
        let ServerConfig {
            tls,
            bind,
            origin,
            assets,
            catalog,
        } = bundle;
        // `tls` paths are only needed to load `config`; `start_server` already validated them.
        let _ = tls;

        // Bind synchronously so `local_addr` is known before we compose the join URL, then hand the
        // listener to the background runtime. `from_tcp_rustls` rejects a blocking listener, so
        // clear the blocking flag first.
        let listener = std::net::TcpListener::bind(bind).map_err(HostFault::Socket)?;
        listener.set_nonblocking(true).map_err(HostFault::Socket)?;
        let addr = listener.local_addr().map_err(HostFault::Socket)?;

        let asset_provider = Arc::clone(&assets);
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let host_epoch = HostEpoch(uuid::Uuid::new_v4());
        let audio_epoch = self.audio_epoch;

        // The DSP bridge publishes arm/mix/disarm confirmations over this bounded channel; the
        // server drains it on the socket loop. `try_send` on a full queue keeps audio non-blocking.
        let (audio_tx, audio_rx) = std::sync::mpsc::sync_channel(AUDIO_EVENT_QUEUE_CAPACITY);
        let event_sink: Arc<dyn crate::contract::AudioEventSink> =
            Arc::new(ChannelAudioEventSink { tx: audio_tx });

        let mut actor = ControlActor::new(
            clock.clone(),
            self.dsp_bridge_with_events(Some(event_sink)),
            catalog.clone(),
            host_epoch,
            audio_epoch,
        );
        // Point the actor's pairing links at the real bound address, then mint the operator's first
        // single-use credential so the returned join URL is immediately usable by a phone.
        actor.set_join_base(format!("https://{addr}"));
        let join_url = actor.issue_pairing_credential().join_url;

        let mut plane = HostServer::with_media_and_catalog(
            actor,
            clock,
            Arc::new(OsEntropy),
            origin,
            assets,
            self.media_hub(),
            catalog,
        );
        plane.set_audio_events(audio_rx);
        // Keep the authoritative server reachable from the returned handle so the operator can
        // mint fresh pairing credentials against the SAME actor the router serves. Without this
        // the actor would be unreachable and credential minting could only be faked.
        let shared = plane.into_shared();
        // The control router serves `/join` but not the bundle's referenced `/assets/*` files.
        // Compose the embedded assets as a fallback so the musician page actually loads in a
        // browser; this wraps the router without changing any `server` internals.
        let router = with_static_assets(HostServer::router_from_shared(Arc::clone(&shared)), asset_provider);

        let handle: axum_server::Handle<SocketAddr> = axum_server::Handle::new();
        let bind_handle = handle.clone();

        // Build the runtime first, then construct the server *inside* that runtime's context.
        // `from_tcp_rustls` converts the std listener with `TcpListener::from_std`, which panics
        // with "there is no reactor running" when called outside a Tokio runtime. Entering the
        // runtime here fixes that while still letting construction errors propagate to the caller
        // (instead of being swallowed on the server thread).
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(HostFault::Socket)?;
        let serve = {
            let _guard = runtime.enter();
            let server = axum_server::from_tcp_rustls(listener, config).map_err(HostFault::Socket)?;
            server.handle(bind_handle)
        };

        let server_thread = thread::Builder::new()
            .name("iem-control-server".to_string())
            .spawn(move || {
                let _ = runtime.block_on(serve.serve(router.into_make_service()));
            })
            .map_err(HostFault::Socket)?;

        Ok(HostServerHandle {
            addr,
            join_url,
            shared,
            handle,
            server_thread: Some(server_thread),
        })
    }
}

/// Load a [`RustlsConfig`] from a TLS identity, surfacing a missing/invalid identity loudly.
fn load_rustls_config(tls: &TlsIdentity) -> Result<RustlsConfig, HostFault> {
    tls.validate_paths()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(HostFault::Socket)?;
    Ok(runtime.block_on(tls.load())?)
}

/// The content type served for an embedded asset path.
///
/// Only a small, explicit allowlist of extensions is recognised; anything else falls back to
/// `application/octet-stream` so an unknown path cannot be coerced into active content.
fn asset_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

/// Whether a requested path is a syntactically safe, single-segment asset key.
///
/// Rejects empty paths, `..` traversal, and any path containing a NUL or backslash. The asset
/// provider is itself keyed by relative path, so this is defence in depth against a traversal key.
fn is_safe_asset_path(raw: &str) -> bool {
    let trimmed = raw.trim_start_matches('/');
    !trimmed.is_empty()
        && !trimmed.contains("..")
        && !trimmed.contains('\0')
        && !trimmed.contains('\\')
}

/// The canonical provider key for the wildcard remainder of `GET /assets/{*path}`.
///
/// The provider is keyed by bundle-relative paths (e.g. `assets/index-HASH.js`), but axum's
/// wildcard extractor yields only the remainder after `/assets/` (e.g. `index-HASH.js`). Restoring
/// the `/assets/` namespace keeps the lookup exact and stable against the provider's contract.
///
/// Returns `None` when the remainder is empty or would escape the namespace.
fn canonical_asset_key(remainder: &str) -> Option<String> {
    if remainder.is_empty() {
        return None;
    }
    let key = format!("assets/{remainder}");
    is_safe_asset_path(&key).then_some(key)
}

/// Serve one embedded asset by key, or a genuine `404` when it is absent.
fn serve_asset(assets: &Arc<dyn AssetProvider>, path: &str) -> Response {
    if !is_safe_asset_path(path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match assets.get(path) {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, asset_content_type(path)),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn asset_handler(
    State(assets): State<Arc<dyn AssetProvider>>,
    Path(path): Path<String>,
) -> Response {
    match canonical_asset_key(&path) {
        Some(key) => serve_asset(&assets, &key),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Wrap the control router so the musician bundle's referenced assets are actually served.
///
/// The control router owns `/join`, pairing, and the WS socket; embedding it here as a fallback
/// keeps every existing route and its authorization untouched. Only `GET /assets/{*path}` is added,
/// and an absent asset is a real `404`, never a fabricated success. This wraps the router instead of
/// editing `server` internals.
fn with_static_assets(router: Router, assets: Arc<dyn AssetProvider>) -> Router {
    let static_routes = Router::new()
        .route("/assets/{*path}", get(asset_handler))
        .with_state(assets);
    static_routes.fallback_service(router)
}

/// Build a draft catalog directly from a channel map (used by the standalone `serve` binary).
pub fn catalog_from_channel_map(
    channel_map: &[ChannelMapEntry],
    revision: CatalogRevision,
) -> CatalogSnapshot {
    let sources = channel_map
        .iter()
        .enumerate()
        .map(|(index, entry)| SourceInfo {
            source_id: entry.source_id,
            physical_index: entry.physical_index,
            label: format!("Channel {}", index + 1),
            role: entry.role,
            authorized: true,
            available: true,
            stereo_pair: entry.stereo_pair,
        })
        .collect();
    CatalogSnapshot {
        catalog_revision: revision,
        sources,
    }
}

/// The media pump: move datagrams both ways and advance session timers.
///
/// `str0m` is Sans-I/O, so the application must: drain outbound datagrams, feed inbound ones, and
/// advance the clock. The loop is intentionally non-blocking so [`HostSession::stop`] can join it
/// promptly; it is also bounded by `MAX_SESSIONS` work per datagram.
fn pump_loop(socket: UdpSocket, hub: Arc<Mutex<MediaHub>>, stopped: Arc<AtomicBool>) {
    let local = match socket.local_addr() {
        Ok(addr) => addr,
        Err(_) => return,
    };
    let mut buf = [0u8; MAX_DATAGRAM_BYTES];

    while !stopped.load(Ordering::Acquire) {
        // 1. Drain outbound datagrams and send them. The lock is released before any syscall.
        let transmits = {
            let mut guard = hub.lock().expect("media hub");
            guard.take_transmits()
        };
        for transmit in transmits {
            let _ = socket.send_to(&transmit.contents, transmit.destination);
        }

        // 2. Feed one inbound datagram, if any, to every open slot. Datagrams that do not belong
        //    to a session are ignored by that session rather than being sent to the wrong peer.
        match socket.recv_from(&mut buf) {
            Ok((len, source)) => {
                let mut guard = hub.lock().expect("media hub");
                for slot in 0..MAX_SESSIONS {
                    if guard.session_at(slot).is_some() {
                        let _ =
                            guard.handle_datagram(slot, Instant::now(), source, local, &buf[..len]);
                    }
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }

        // 3. Advance every session's timers on a deadline.
        {
            let mut guard = hub.lock().expect("media hub");
            guard.tick(Instant::now());
        }

        thread::sleep(PUMP_IDLE);
    }
}

/// Build a default channel map: `channels` input channels centered to stereo.
///
/// This is a draft POC mapping (each physical input becomes an independent source); the operator's
/// real device capability probe is what finalizes source roles.
pub fn default_channel_map(channels: u16) -> Vec<ChannelMapEntry> {
    (0..channels)
        .map(|index| ChannelMapEntry {
            physical_index: index,
            source_id: SourceId::from_bytes([index as u8; 16]),
            stereo_pair: None,
            role: SourceRole::InputChannel,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::service::{CaptureCapabilities, CaptureSink, CaptureStream};
    use crate::contract::{CaptureBlock, EncodedFrame, SampleFormat};
    use crate::ids::{SessionEpoch, MAX_BLOCK_FRAMES};
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    struct PushingBackend {
        channels: u16,
        frames: u32,
        blocks: u32,
    }

    impl CaptureBackend for PushingBackend {
        fn capabilities(
            &self,
            _req: &crate::contract::CaptureRequest,
        ) -> Result<CaptureCapabilities, CaptureFault> {
            Ok(CaptureCapabilities {
                channels: self.channels,
                sample_rate_hz: 48_000,
                buffer_frames: self.frames,
                sample_format: SampleFormat::F32,
            })
        }

        fn build(
            &self,
            _req: &crate::contract::CaptureRequest,
            _caps: &CaptureCapabilities,
            sink: CaptureSink,
        ) -> Result<Box<dyn CaptureStream>, CaptureFault> {
            Ok(Box::new(PusherStream {
                sink: Some(sink),
                channels: self.channels,
                frames: self.frames.min(MAX_BLOCK_FRAMES as u32),
                blocks: self.blocks,
            }))
        }
    }

    struct PusherStream {
        sink: Option<CaptureSink>,
        channels: u16,
        frames: u32,
        blocks: u32,
    }

    impl CaptureStream for PusherStream {
        fn play(&mut self) -> Result<(), CaptureFault> {
            let Some(mut sink) = self.sink.take() else {
                return Ok(());
            };
            let channels = self.channels;
            let frames = self.frames;
            let blocks = self.blocks;
            thread::spawn(move || {
                for _ in 0..blocks {
                    if let Ok(mut slot) = sink.free_rx.pop() {
                        for (i, sample) in slot.iter_mut().enumerate() {
                            *sample = if i % channels as usize == 0 { 0.4 } else { 0.0 };
                        }
                        let block = CaptureBlock {
                            audio_epoch: sink.audio_epoch,
                            start_sample: 0,
                            frame_count: frames,
                            sample_rate_hz: sink.sample_rate_hz,
                            channel_count: channels,
                            channel_map_revision: sink.channel_map_revision,
                            samples: slot,
                        };
                        let _ = sink.ready_tx.push(block);
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            });
            Ok(())
        }
    }

    fn loopback() -> SelectedInterface {
        SelectedInterface {
            name: "lo0".to_string(),
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            prefix: 8,
            // 0 lets the host bind an ephemeral port and advertise the real one.
            port: 0,
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("iem-cast-{tag}-{}.pem", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("temp file");
        file.write_all(b"test").expect("write temp file");
        path
    }

    fn valid_tls() -> TlsIdentity {
        TlsIdentity::new(temp_file("cert"), temp_file("key"))
    }

    fn start_request() -> StartHostRequest {
        StartHostRequest {
            capture: crate::contract::CaptureRequest {
                device_id: "fake".to_string(),
                sample_rate_hz: 48_000,
                buffer_frames: 128,
            },
            interface: crate::contract::InterfaceInfo {
                name: "lo0".to_string(),
                ip_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                prefix: 8,
            },
            certificate_path: temp_file("cert").display().to_string(),
            key_path: temp_file("key").display().to_string(),
        }
    }

    #[test]
    fn host_session_starts_with_a_fake_backend_and_stops_cleanly() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 20,
        };
        let mut session = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        // The media socket binds a real loopback port; no listener slot exists yet.
        assert!(session.local_addr().is_ok());
        assert_eq!(session.media_hub().lock().unwrap().session_at(0), None);

        session.stop();
        session.stop(); // idempotent
    }

    #[test]
    fn host_advertises_the_real_bound_media_port_not_zero() {
        // Regression: the WebRTC host candidate used to be advertised at port 0 while the UDP
        // socket bound an ephemeral port afterwards. The phone was told to reach `ip:0`, so ICE
        // could never connect and the receiver reported "WebRTC connection failed".
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 0,
        };
        let session = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        let bound = session.local_addr().expect("media socket bound").port();
        let advertised = session.media_hub().lock().unwrap().interface().port;

        assert_ne!(bound, 0, "the media socket must bind a real port");
        assert_eq!(
            advertised, bound,
            "the advertised candidate port must match the bound socket"
        );

        drop(session);
    }

    #[test]
    fn host_session_rejects_an_invalid_tls_identity() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 1,
        };
        let missing = TlsIdentity::new("/nonexistent/cert.pem", "/nonexistent/key.pem");
        let result = HostSession::start(
            &backend,
            missing,
            loopback(),
            start_request(),
            default_channel_map(1),
        );
        assert!(matches!(result, Err(HostFault::Tls(_))));
    }

    #[test]
    fn media_hub_sink_routes_blocks_into_the_shared_hub() {
        let hub = Arc::new(Mutex::new(MediaHub::new(loopback())));
        let session = SessionEpoch::from_bytes([0x22; 16]);
        hub.lock().unwrap().open_slot(0, session).unwrap();

        let sink = MediaHubSink {
            hub: Arc::clone(&hub),
        };
        let mut outputs: [ListenerOutput; MAX_SESSIONS] =
            std::array::from_fn(ListenerOutput::empty);
        outputs[0] = ListenerOutput {
            slot: 0,
            session,
            count: 1,
            frames: [EncodedFrame::zeroed(); crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };
        outputs[1] = ListenerOutput {
            slot: 1,
            session: SessionEpoch::from_bytes([0x23; 16]),
            count: 1,
            frames: [EncodedFrame::zeroed(); crate::audio::MAX_OUT_FRAMES_PER_BLOCK],
        };

        // No panic and no transmit while the gate is closed / the peer is not connected.
        sink.on_block(&outputs);
        assert!(hub.lock().unwrap().take_transmits().is_empty());
    }

    #[test]
    fn default_channel_map_is_centered_input_channels() {
        let map = default_channel_map(2);
        assert_eq!(map.len(), 2);
        assert_eq!(map[1].physical_index, 1);
        assert_eq!(map[1].role, SourceRole::InputChannel);
    }

    #[test]
    fn dsp_bridge_queues_a_mix_that_precedes_the_media_slot_then_applies_on_arm() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 5,
        };
        let session = SessionEpoch::from_bytes([0x22; 16]);
        let host = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        let bridge = host.dsp_bridge();

        // A patch arriving BEFORE the media slot exists must be accepted and queued, not lost.
        let snapshot = crate::contract::MixSnapshot {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(0),
            },
            catalog_revision: crate::ids::CatalogRevision(1),
            mix_revision: crate::ids::MixRevision(1),
            sources: crate::contract::SourceGainMatrix::from_slice(&[crate::contract::SourceGain {
                source_id: SourceId::from_bytes([0x11; 16]),
                gain_db: -6.0,
                muted: true,
            }]),
            master_db: 0.0,
            master_muted: true,
        };
        assert!(crate::contract::DspControl::install_snapshot(&*bridge, snapshot).is_ok());

        // Now signaling opens the media slot for the same session.
        host.media_hub().lock().unwrap().open_slot(0, session).unwrap();

        let arm = crate::contract::ListenArm {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(1),
            },
            applied_revision: crate::ids::MixRevision(1),
            arm_nonce: crate::ids::ArmNonce::from_bytes([0x66; 16]),
        };
        // Arming flushes the queued mix and opens the session's gate; must not panic or error.
        assert!(crate::contract::DspControl::request_arm(&*bridge, arm).is_ok());

        // A subsequent patch now routes straight through (slot exists).
        assert!(crate::contract::DspControl::install_snapshot(&*bridge, snapshot).is_ok());

        // Disarm is priority and must not panic.
        crate::contract::DspControl::disarm(&*bridge, session, crate::ids::SafetyGeneration(2));
    }

    /// Records every event the DSP bridge publishes.
    #[derive(Default)]
    struct RecordingEvents {
        events: Mutex<Vec<AudioEvent>>,
    }

    impl crate::contract::AudioEventSink for RecordingEvents {
        fn try_publish(&self, ev: AudioEvent) -> bool {
            self.events.lock().expect("events").push(ev);
            true
        }
    }

    #[test]
    fn dsp_bridge_publishes_arm_mix_and_disarm_events_to_its_sink() {
        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 5,
        };
        let session = SessionEpoch::from_bytes([0x44; 16]);
        let host = HostSession::start(
            &backend,
            valid_tls(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        let sink = Arc::new(RecordingEvents::default());
        let bridge = host.dsp_bridge_with_events(Some(sink.clone()));

        // A media slot exists, so an install routes straight to the runtime and publishes.
        host.media_hub().lock().unwrap().open_slot(0, session).unwrap();
        let snapshot = crate::contract::MixSnapshot {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(0),
            },
            catalog_revision: crate::ids::CatalogRevision(1),
            mix_revision: crate::ids::MixRevision(1),
            sources: crate::contract::SourceGainMatrix::from_slice(&[]),
            master_db: 0.0,
            master_muted: true,
        };
        assert!(crate::contract::DspControl::install_snapshot(&*bridge, snapshot).is_ok());

        let arm = crate::contract::ListenArm {
            context: crate::contract::SessionContext {
                session_epoch: session,
                audio_epoch: host.audio_epoch(),
                safety_generation: crate::ids::SafetyGeneration(1),
            },
            applied_revision: crate::ids::MixRevision(1),
            arm_nonce: crate::ids::ArmNonce::from_bytes([0x66; 16]),
        };
        assert!(crate::contract::DspControl::request_arm(&*bridge, arm).is_ok());
        crate::contract::DspControl::disarm(&*bridge, session, crate::ids::SafetyGeneration(2));

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            AudioEvent::MixApplied {
                applied_revision: crate::ids::MixRevision(1),
                ..
            }
        ));
        assert!(matches!(
            events[1],
            AudioEvent::ArmApplied { arm_nonce, .. }
                if arm_nonce == crate::ids::ArmNonce::from_bytes([0x66; 16])
        ));
        assert!(matches!(
            events[2],
            AudioEvent::Interrupted {
                session_epoch,
                reason: InterruptReason::UserDisarm,
            } if session_epoch == session
        ));
    }

    /// Write a fresh self-signed localhost certificate/key pair and return their paths.
    ///
    /// Generated offline with `rcgen` so the server-start regression test never touches the
    /// network, the operator's real `local-certs/`, or `mkcert`.
    fn generated_tls_files() -> (PathBuf, PathBuf) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("generate self-signed certificate");
        let dir = std::env::temp_dir();
        let cert_path = dir.join(format!("iem-cast-test-{}-cert.pem", std::process::id()));
        let key_path = dir.join(format!("iem-cast-test-{}-key.pem", std::process::id()));
        std::fs::write(&cert_path, cert.cert.pem()).expect("write test certificate");
        std::fs::write(&key_path, cert.signing_key.serialize_pem()).expect("write test key");
        (cert_path, key_path)
    }

    #[test]
    fn starting_the_control_server_binds_without_an_ambient_tokio_runtime() {
        // Regression: `axum_server::from_tcp_rustls` converts the std listener with
        // `TcpListener::from_std`, which panics with "there is no reactor running" when the server
        // is constructed outside a Tokio runtime. This test calls that path from a plain test
        // thread (no ambient runtime) with a REAL rustls config, so the previous bug aborts the
        // process here instead of only on a developer's machine.
        let (cert_path, key_path) = generated_tls_files();
        let tls = TlsIdentity::new(cert_path, key_path);

        // Build the rustls config exactly as production does, in its own runtime.
        let config = load_rustls_config(&tls).expect("loads the generated certificate");

        let backend = PushingBackend {
            channels: 1,
            frames: 128,
            blocks: 0,
        };
        let session = HostSession::start(
            &backend,
            tls.clone(),
            loopback(),
            start_request(),
            default_channel_map(1),
        )
        .expect("host session starts");

        let bundle = ServerConfig {
            tls,
            // Port 0 lets the OS pick a free port so the test never collides with a running host.
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            origin: OriginPolicy::new(vec!["https://localhost".to_string()]),
            assets: Arc::new(OneAsset),
            catalog: catalog_from_channel_map(&default_channel_map(1), CatalogRevision(0)),
        };

        let server = session
            .start_server_with_config(config, bundle)
            .expect("control server starts without an ambient runtime");
        // A real bound port and a real single-use join URL are the observable proof it started.
        assert!(server.local_addr().port() != 0);
        assert!(server.join_url().starts_with("https://"));

        // A freshly minted credential must be a real token, and must differ each time (single-use).
        let first = server.issue_pairing_credential().expect("mints a credential");
        let second = server.issue_pairing_credential().expect("mints another credential");
        assert!(first.join_url.contains("t="));
        assert_ne!(first.join_url, second.join_url, "tokens must be single-use");

        drop(server);
    }

    /// A minimal asset provider with one real asset and no entry for unknown paths.
    struct OneAsset;

    impl AssetProvider for OneAsset {
        fn get(&self, path: &str) -> Option<&'static [u8]> {
            match path {
                "assets/app.js" | "/assets/app.js" => Some(b"console.log(1);".as_slice()),
                _ => None,
            }
        }
    }

    #[test]
    fn asset_route_serves_a_present_asset_and_404s_an_absent_one() {
        let provider: Arc<dyn AssetProvider> = Arc::new(OneAsset);

        let present = serve_asset(&provider, "assets/app.js");
        assert_eq!(present.status(), StatusCode::OK);

        // An absent asset is a real 404, never a fabricated success.
        let absent = serve_asset(&provider, "assets/missing.js");
        assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn asset_path_traversal_and_unknown_extensions_are_rejected() {
        assert!(!is_safe_asset_path("assets/../../etc/passwd"));
        assert!(!is_safe_asset_path(""));
        assert!(!is_safe_asset_path("assets\\secret"));
        assert!(is_safe_asset_path("/assets/app.js"));

        // An unknown extension still yields a defined, inert content type rather than active HTML.
        assert_eq!(
            asset_content_type("assets/thing.bin"),
            "application/octet-stream"
        );
        assert_eq!(
            asset_content_type("assets/app.js"),
            "text/javascript; charset=utf-8"
        );
    }

    /// The router composition must compile and be constructible (it is the wire-in used by
    /// `start_server`), without asserting any server-start behavior.
    #[test]
    fn static_asset_router_composes_without_panicking() {
        let _router = with_static_assets(Router::new(), Arc::new(OneAsset));
    }

    /// A provider that serves **only** the canonical namespace key `assets/app.js`, mirroring
    /// `EmbeddedMusicianAssets`, which strips a leading slash but keeps the `assets/` prefix.
    struct CanonicalOnlyAsset;

    impl AssetProvider for CanonicalOnlyAsset {
        fn get(&self, path: &str) -> Option<&'static [u8]> {
            (path == "assets/app.js").then_some(b"console.log(1);".as_slice())
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn asset_handler_serves_the_canonical_namespace_key_with_js_mime() {
        let provider: Arc<dyn AssetProvider> = Arc::new(CanonicalOnlyAsset);

        // `Path` for `GET /assets/app.js` on route `/assets/{*path}` extracts the wildcard
        // remainder exactly: `app.js` (no leading `assets/`). The handler must still resolve the
        // canonical provider key `assets/app.js`.
        let response = block_on(asset_handler(State(provider.clone()), Path("app.js".to_string())));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/javascript; charset=utf-8")
        );
        let body = block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
            .expect("read body");
        assert_eq!(&body[..], b"console.log(1);");

        // A missing canonical asset is a real 404.
        let absent = block_on(asset_handler(State(provider.clone()), Path("missing.js".to_string())));
        assert_eq!(absent.status(), StatusCode::NOT_FOUND);

        // Traversal in the wildcard remainder is still rejected.
        let traversal = block_on(asset_handler(
            State(provider),
            Path("../etc/passwd".to_string()),
        ));
        assert_eq!(traversal.status(), StatusCode::NOT_FOUND);
    }
}
