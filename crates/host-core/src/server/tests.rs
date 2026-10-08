//! Lane B server tests: pairing, cookie/origin authorization, and the POC receiver cap.
//!
//! These tests exercise the real HTTP boundary functions, not `ControlActor` directly, so the
//! server-boundary authorization claims are actually under test.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::contract::{
    AssetProvider, AudioEvent, CatalogSnapshot, Clock, ControlError, DspControl, EnvelopeV1,
    ListenArm, MixPatch, MixSnapshot, SessionContext, SourceGain, SourceGainMatrix, SourceInfo,
    SourceRole,
};
use crate::control::auth::OriginPolicy;
use crate::control::ControlActor;
use crate::ids::{
    ArmNonce, AudioEpoch, CatalogRevision, HostEpoch, MixRevision, RequestId, SafetyGeneration,
    SessionEpoch, SourceId,
};
use crate::server::http::{HostServer, PairRequest};
use crate::server::pairing::PairStore;
use crate::server::protocol::{ProtocolError, ServerMessage};

// ---------------------------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct TestClock {
    now: Mutex<Instant>,
}

impl TestClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(Instant::now()),
        })
    }

    pub fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("clock");
        *now += duration;
    }

    pub fn advance_secs(&self, secs: u64) {
        self.advance(Duration::from_secs(secs));
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.now.lock().expect("clock")
    }
}

/// Deterministic entropy for tests.
pub struct TestEntropy {
    counter: Mutex<u64>,
}

impl TestEntropy {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            counter: Mutex::new(1),
        })
    }
}

impl crate::contract::Entropy for TestEntropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), crate::contract::EntropyError> {
        let mut counter = self.counter.lock().expect("entropy");
        for byte in buf.iter_mut() {
            *byte = (*counter & 0xff) as u8;
            *counter = counter.wrapping_add(1);
        }
        Ok(())
    }
}

/// Recording DSP double.
#[derive(Default)]
pub struct RecordingDsp {
    pub installed: Mutex<Vec<MixSnapshot>>,
    pub disarms: Mutex<Vec<(SessionEpoch, SafetyGeneration)>>,
    pub arms: Mutex<Vec<ListenArm>>,
}

impl DspControl for RecordingDsp {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError> {
        self.installed.lock().expect("dsp").push(snapshot);
        Ok(())
    }

    fn request_arm(&self, arm: ListenArm) -> Result<(), ControlError> {
        self.arms.lock().expect("dsp").push(arm);
        Ok(())
    }

    fn disarm(&self, session: SessionEpoch, new_generation: SafetyGeneration) {
        self.disarms
            .lock()
            .expect("dsp")
            .push((session, new_generation));
    }
}

struct FixtureAssets;

impl AssetProvider for FixtureAssets {
    fn get(&self, path: &str) -> Option<&'static [u8]> {
        (path == "/join").then_some(b"<html>musician join</html>".as_slice())
    }
}

pub fn shared_source() -> SourceId {
    SourceId::from_bytes([0x33; 16])
}

pub fn unavailable_source() -> SourceId {
    SourceId::from_bytes([0x99; 16])
}

pub fn test_catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog_revision: CatalogRevision(1),
        sources: vec![
            SourceInfo {
                source_id: shared_source(),
                physical_index: 1,
                label: "Shared".to_string(),
                role: SourceRole::InputChannel,
                authorized: true,
                available: true,
                stereo_pair: None,
            },
            SourceInfo {
                source_id: unavailable_source(),
                physical_index: 2,
                label: "Offline".to_string(),
                role: SourceRole::InputChannel,
                authorized: true,
                available: false,
                stereo_pair: None,
            },
        ],
    }
}

pub fn test_host_epoch() -> HostEpoch {
    HostEpoch::from_bytes([0x11; 16])
}

pub fn test_audio_epoch() -> AudioEpoch {
    AudioEpoch::from_bytes([0x55; 16])
}

pub fn test_session_a() -> SessionEpoch {
    SessionEpoch::from_bytes([0x22; 16])
}

pub fn control_actor_with(clock: Arc<dyn Clock>, dsp: Arc<dyn DspControl>) -> ControlActor {
    ControlActor::new(
        clock,
        dsp,
        test_catalog(),
        test_host_epoch(),
        test_audio_epoch(),
    )
}

/// A native gain-only patch (master stays muted to avoid the unarmed-unmute gate).
pub fn patch_source_gain(source_id: SourceId, gain_db: f32) -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id,
            gain_db,
            muted: true,
        }]),
        master_db: -6.0,
        master_muted: true,
    }
}

/// Serialize a `mix.patch` envelope for the HTTP boundary.
pub fn mix_envelope(session: Option<SessionEpoch>, patch: MixPatch) -> Vec<u8> {
    let envelope = EnvelopeV1 {
        v: 1,
        kind: "mix.patch".to_string(),
        request_id: RequestId("r".to_string()),
        host_epoch: test_host_epoch(),
        session_epoch: session,
        payload: patch,
    };
    serde_json::to_vec(&envelope).unwrap()
}

fn build_server() -> (HostServer, Arc<dyn Clock>) {
    let clock: Arc<dyn Clock> = TestClock::new();
    let dsp = Arc::new(RecordingDsp::default());
    let control = control_actor_with(clock.clone(), dsp);
    let entropy: Arc<dyn crate::contract::Entropy> = TestEntropy::new();
    let origin = OriginPolicy::new(["https://host.local:8443".to_string()]);
    let server = HostServer::new(control, clock.clone(), entropy, origin, Arc::new(FixtureAssets));
    (server, clock)
}

pub fn test_server_with_two_pairings() -> (HostServer, SessionEpoch, SessionEpoch, String, String) {
    let (mut server, _clock) = build_server();
    let (cookie_a, session_a) = pair(&mut server);
    let (cookie_b, session_b) = pair(&mut server);
    (server, session_a, session_b, cookie_a, cookie_b)
}

fn token_of(join_url: &str) -> String {
    join_url
        .split_once("#t=")
        .expect("token fragment")
        .1
        .to_string()
}

fn pair(server: &mut HostServer) -> (String, SessionEpoch) {
    let credential = server.control.issue_pairing_credential();
    let token = token_of(&credential.join_url);
    let body = serde_json::to_vec(&PairRequest { token }).unwrap();
    let (cookie, _payload) = server.handle_pair(&body).expect("pair");
    let cookie_value = cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string();
    let session = server.sessions.authenticate(&cookie_value).expect("session");
    (cookie_value, session)
}

// ---------------------------------------------------------------------------------------------
// Pairing tests
// ---------------------------------------------------------------------------------------------

#[test]
fn pairing_token_is_256bit_single_use_and_expires_at_120s() {
    let clock = TestClock::new();
    let mut store = PairStore::new(TestEntropy::new(), clock.clone());
    let t = store.issue();
    assert_eq!(t.len(), 64); // 32 bytes of entropy, hex-encoded
    assert!(store.exchange(&t).is_ok());
    assert!(store.exchange(&t).is_err()); // one use

    let t2 = store.issue();
    clock.advance_secs(121);
    assert!(store.exchange(&t2).is_err());
}

#[test]
fn pairing_credentials_are_never_logged() {
    let clock = TestClock::new();
    let mut store = PairStore::new(TestEntropy::new(), clock);
    let t = store.issue();
    // The store's Debug output must never carry a live token.
    let rendered = format!("{store:?}");
    assert!(!rendered.contains(&t));
    let credential = store.issue_credential();
    assert!(credential.join_url.starts_with("https://"));
}

#[test]
fn issue_pairing_credential_returns_join_url_and_expiry() {
    let mut actor = control_actor_with(TestClock::new(), Arc::new(RecordingDsp::default()));
    let c = actor.issue_pairing_credential();
    assert!(c.join_url.starts_with("https://"));
    assert_eq!(c.expires_in_seconds, 120);
}

#[test]
fn ws_requires_strict_origin_before_upgrade() {
    let (server, _) = build_server();
    assert_eq!(
        crate::server::http::origin_gate(&server, "https://host.local:8443"),
        101
    );
    assert_eq!(
        crate::server::http::origin_gate(&server, "https://evil.example"),
        403
    );
}

#[test]
fn session_cookie_is_secure_httponly_samesite_strict() {
    let (mut server, _) = build_server();
    let credential = server.control.issue_pairing_credential();
    let token = token_of(&credential.join_url);
    let body = serde_json::to_vec(&PairRequest { token }).unwrap();
    let (cookie, _) = server.handle_pair(&body).expect("pair");
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Max-Age=43200"));
}

#[test]
fn two_musicians_can_listen_to_the_same_source_with_independent_settings() {
    let (mut server, session_a, session_b, cookie_a, cookie_b) = test_server_with_two_pairings();

    // Both patches go through the real HTTP boundary using each cookie.
    let body_a = mix_envelope(
        Some(session_a),
        patch_source_gain(shared_source(), -3.0),
    );
    let body_b = mix_envelope(
        Some(session_b),
        patch_source_gain(shared_source(), -9.0),
    );
    assert_eq!(server.handle_mix(Some(&cookie_a), &body_a).unwrap().0, 200);
    assert_eq!(server.handle_mix(Some(&cookie_b), &body_b).unwrap().0, 200);

    assert_eq!(
        server.accepted_gain_for_cookie(&cookie_a, shared_source()),
        Some(-3.0)
    );
    assert_eq!(
        server.accepted_gain_for_cookie(&cookie_b, shared_source()),
        Some(-9.0)
    );
}

#[test]
fn unavailable_source_is_forbidden_within_logical_permissions() {
    let clock: Arc<dyn Clock> = TestClock::new();
    let mut actor = control_actor_with(clock, Arc::new(RecordingDsp::default()));
    let result = actor.apply_patch(
        test_session_a(),
        patch_source_gain(unavailable_source(), -3.0),
    );
    assert_eq!(result, Err(ControlError::SourceForbidden));
}

#[test]
fn cross_mix_session_id_is_rejected_at_the_server_boundary() {
    let (mut server, _session_a, session_b, cookie_a, _cookie_b) = test_server_with_two_pairings();

    // cookieA presents an envelope claiming sessionB.
    let body = mix_envelope(Some(session_b), patch_source_gain(shared_source(), -3.0));
    let result = server.handle_mix(Some(&cookie_a), &body);
    assert_eq!(result, Err(ControlError::Unauthorized));
}

#[test]
fn server_message_envelope_serializes_kind_and_session() {
    let message = ServerMessage::ProtocolError(ProtocolError {
        code: "REVISION_CONFLICT".to_string(),
        message: "revision conflict".to_string(),
        retryable: true,
    });
    let json = message.to_envelope_json(test_host_epoch(), RequestId("r1".to_string()));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["type"], "error");
    assert_eq!(value["payload"]["code"], "REVISION_CONFLICT");
    assert_eq!(value["hostEpoch"], test_host_epoch().to_string());
}

#[test]
fn poc_receiver_cap_replaces_oldest_receiver() {
    let (mut server, _session_a, _session_b, cookie_a, _cookie_b) = test_server_with_two_pairings();

    // A third valid pairing must evict the oldest receiver rather than exceed the cap.
    let (_cookie_c, _) = pair(&mut server);

    assert_eq!(server.authorize(Some(&cookie_a)), Err(ControlError::Unauthorized));
    assert_eq!(server.sessions.active_count(), 2);
}

#[test]
fn join_page_serves_musician_html_not_admin() {
    let (server, _) = build_server();
    let bytes = server.asset("/join").expect("join asset");
    assert!(std::str::from_utf8(bytes).unwrap().contains("musician"));
}

// ---------------------------------------------------------------------------------------------
// RTC signaling
// ---------------------------------------------------------------------------------------------

#[test]
fn rtc_dtos_are_camel_case_and_round_trip() {
    let offer = crate::contract::RtcOffer {
        sdp: "v=0".to_string(),
    };
    let value = serde_json::to_value(&offer).unwrap();
    assert_eq!(value["sdp"], "v=0");

    let candidate: crate::contract::RtcCandidate = serde_json::from_str(
        r#"{"candidate":null,"sdpMid":"0","sdpMLineIndex":0}"#,
    )
    .unwrap();
    assert_eq!(candidate.candidate, None);
    assert_eq!(candidate.sdp_mid.as_deref(), Some("0"));
    assert_eq!(candidate.sdp_m_line_index, Some(0));
    let back = serde_json::to_value(&candidate).unwrap();
    assert_eq!(back["sdpMid"], "0");
    assert_eq!(back["sdpMLineIndex"], 0);

    let answer = crate::contract::RtcAnswer {
        sdp: "v=0".to_string(),
    };
    assert_eq!(serde_json::to_value(&answer).unwrap()["sdp"], "v=0");
}

#[test]
fn rtc_server_message_kinds_and_session_scoping() {
    let answer = ServerMessage::RtcAnswer(crate::contract::RtcAnswer {
        sdp: "v=0".to_string(),
    });
    assert_eq!(answer.kind(), "rtc.answer");
    assert_eq!(answer.session_epoch(), None);
    let json = answer.to_envelope_json_with_session(
        test_host_epoch(),
        RequestId("r1".to_string()),
        Some(test_session_a()),
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["type"], "rtc.answer");
    assert_eq!(value["payload"]["sdp"], "v=0");
    assert_eq!(value["sessionEpoch"], test_session_a().to_string());

    let candidate = ServerMessage::RtcCandidate(crate::contract::RtcCandidate {
        candidate: None,
        sdp_mid: Some("0".to_string()),
        sdp_m_line_index: Some(0),
    });
    assert_eq!(candidate.kind(), "rtc.candidate");
}

fn rtc_envelope(kind: &str, payload: serde_json::Value) -> String {
    serde_json::to_string(&EnvelopeV1 {
        v: 1,
        kind: kind.to_string(),
        request_id: RequestId("rtc-1".to_string()),
        host_epoch: test_host_epoch(),
        session_epoch: Some(test_session_a()),
        payload,
    })
    .unwrap()
}

#[test]
fn ws_rtc_candidate_opens_the_authenticated_slot_and_acks() {
    let (server, _clock) = build_server();
    let shared: crate::server::http::SharedServer = Arc::new(Mutex::new(server));

    let text = rtc_envelope(
        "rtc.candidate",
        serde_json::json!({"candidate": null, "sdpMid": "0", "sdpMLineIndex": 0}),
    );
    let reply = crate::server::ws::dispatch(&shared, test_session_a(), &text).expect("reply");
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(value["type"], "rtc.candidate");
    assert!(value["payload"]["candidate"].is_null());
    assert_eq!(value["sessionEpoch"], test_session_a().to_string());

    // The candidate opened a media slot bound to the authenticated session.
    let guard = shared.lock().unwrap();
    let hub = guard.hub.lock().unwrap();
    assert_eq!(hub.session_at(0), Some(test_session_a()));
}

#[test]
fn ws_rtc_offer_with_a_malformed_sdp_is_rejected() {
    let (server, _clock) = build_server();
    let shared: crate::server::http::SharedServer = Arc::new(Mutex::new(server));

    let text = rtc_envelope("rtc.offer", serde_json::json!({"sdp": "not sdp"}));
    let reply = crate::server::ws::dispatch(&shared, test_session_a(), &text).expect("reply");
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(value["type"], "error");
    assert_eq!(value["payload"]["code"], "INTERNAL");
}

#[test]
fn ws_cross_session_rtc_message_is_rejected() {
    let (server, _clock) = build_server();
    let shared: crate::server::http::SharedServer = Arc::new(Mutex::new(server));

    let body = serde_json::to_string(&EnvelopeV1 {
        v: 1,
        kind: "rtc.candidate".to_string(),
        request_id: RequestId("rtc-2".to_string()),
        host_epoch: test_host_epoch(),
        session_epoch: Some(test_session_a()),
        payload: serde_json::json!({"candidate": null}),
    })
    .unwrap();
    // The socket is authenticated as a *different* session.
    let other = SessionEpoch::from_bytes([0x77; 16]);
    let reply = crate::server::ws::dispatch(&shared, other, &body).expect("reply");
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(value["type"], "error");
    assert_eq!(value["payload"]["code"], "UNAUTHORIZED");
}

// ---------------------------------------------------------------------------------------------
// Outbound push: initial snapshots and audio-event delivery
// ---------------------------------------------------------------------------------------------

/// Build a server whose catalog is the fixture catalog (for `catalog.snapshot` assertions).
fn build_server_with_catalog() -> HostServer {
    let clock: Arc<dyn Clock> = TestClock::new();
    let dsp = Arc::new(RecordingDsp::default());
    let control = control_actor_with(clock.clone(), dsp);
    let entropy: Arc<dyn crate::contract::Entropy> = TestEntropy::new();
    let origin = OriginPolicy::new(["https://host.local:8443".to_string()]);
    let hub = Arc::new(Mutex::new(crate::transport::MediaHub::new(
        crate::transport::SelectedInterface {
            name: "loopback".to_string(),
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            prefix: 8,
            port: 0,
        },
    )));
    HostServer::with_media_and_catalog(
        control,
        clock,
        entropy,
        origin,
        Arc::new(FixtureAssets),
        hub,
        test_catalog(),
    )
}

/// Drain all currently queued outbound envelopes, parsed as JSON.
fn drain_all(
    receiver: &mut tokio::sync::mpsc::Receiver<String>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(text) = receiver.try_recv() {
        out.push(serde_json::from_str(&text).expect("outbound envelope is json"));
    }
    out
}

#[test]
fn connect_session_pushes_session_and_catalog_snapshots_in_order() {
    let mut server = build_server_with_catalog();
    let session = test_session_a();
    let (mut rx, _tx) = server.connect_session(session);

    let messages = drain_all(&mut rx);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["type"], "session.snapshot");
    assert_eq!(
        messages[0]["payload"]["sessionEpoch"],
        test_session_a().to_string()
    );
    assert_eq!(messages[0]["payload"]["phase"], "paired");
    assert!(messages[0]["payload"]["acceptedMix"].is_null());

    assert_eq!(messages[1]["type"], "catalog.snapshot");
    assert_eq!(messages[1]["payload"]["catalogRevision"], "1");
    assert_eq!(messages[1]["payload"]["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn publish_mix_applied_reaches_only_the_owning_session() {
    let (mut server, _clock) = build_server();
    let session = test_session_a();
    let other = SessionEpoch::from_bytes([0x77; 16]);

    // Give the actor a known installed revision for `session`.
    server
        .control
        .apply_patch(session, patch_source_gain(shared_source(), -3.0))
        .expect("patch");

    let (mut rx_a, _tx_a) = server.connect_session(session);
    let (mut rx_b, _tx_b) = server.connect_session(other);
    drain_all(&mut rx_a);
    drain_all(&mut rx_b);

    let snapshot = server.control.accepted_snapshot(session).expect("accepted");
    server.publish_audio_event(AudioEvent::MixApplied {
        applied_revision: MixRevision(1),
        context: SessionContext {
            session_epoch: session,
            audio_epoch: test_audio_epoch(),
            safety_generation: SafetyGeneration(0),
        },
        start_sample: 0,
        snapshot,
    });

    let pushed = drain_all(&mut rx_a);
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0]["type"], "mix.applied");
    assert_eq!(pushed[0]["payload"]["appliedRevision"], "1");
    // The other registered session must not receive the owner's push.
    assert!(drain_all(&mut rx_b).is_empty());
}

#[test]
fn publish_arm_applied_reaches_only_the_owning_session() {
    let (mut server, _clock) = build_server();
    let session = test_session_a();
    let other = SessionEpoch::from_bytes([0x77; 16]);
    let nonce = ArmNonce::from_bytes([0x66; 16]);

    server
        .control
        .arm(
            session,
            ListenArm {
                context: SessionContext {
                    session_epoch: session,
                    audio_epoch: test_audio_epoch(),
                    safety_generation: SafetyGeneration(0),
                },
                applied_revision: MixRevision(0),
                arm_nonce: nonce,
            },
        )
        .expect("arm");

    let (mut rx_a, _tx_a) = server.connect_session(session);
    let (mut rx_b, _tx_b) = server.connect_session(other);
    drain_all(&mut rx_a);
    drain_all(&mut rx_b);

    server.publish_audio_event(AudioEvent::ArmApplied {
        context: SessionContext {
            session_epoch: session,
            audio_epoch: test_audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: nonce,
    });

    let pushed = drain_all(&mut rx_a);
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0]["type"], "listen.armed");
    assert_eq!(pushed[0]["payload"]["armNonce"], nonce.to_string());
    assert!(drain_all(&mut rx_b).is_empty());
}

#[test]
fn stale_and_wrong_session_audio_events_are_never_pushed() {
    let (mut server, _clock) = build_server();
    let session = test_session_a();
    let other = SessionEpoch::from_bytes([0x77; 16]);

    let (mut rx_a, _tx_a) = server.connect_session(session);
    let (mut rx_b, _tx_b) = server.connect_session(other);
    drain_all(&mut rx_a);
    drain_all(&mut rx_b);

    // An event naming a session the actor has never seen is dropped.
    server.publish_audio_event(AudioEvent::ArmApplied {
        context: SessionContext {
            session_epoch: other,
            audio_epoch: test_audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: ArmNonce::from_bytes([0x66; 16]),
    });
    assert!(drain_all(&mut rx_a).is_empty());
    assert!(drain_all(&mut rx_b).is_empty());

    // A stale (uninstalled) revision for a known session is dropped.
    let nonce = ArmNonce::from_bytes([0x33; 16]);
    server
        .control
        .apply_patch(session, patch_source_gain(shared_source(), -3.0))
        .expect("patch");
    server
        .control
        .arm(
            session,
            ListenArm {
                context: SessionContext {
                    session_epoch: session,
                    audio_epoch: test_audio_epoch(),
                    safety_generation: SafetyGeneration(0),
                },
                applied_revision: MixRevision(1),
                arm_nonce: nonce,
            },
        )
        .expect("arm");

    let snapshot = server.control.accepted_snapshot(session).expect("accepted");
    server.publish_audio_event(AudioEvent::MixApplied {
        applied_revision: MixRevision(9999),
        context: SessionContext {
            session_epoch: session,
            audio_epoch: test_audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        start_sample: 0,
        snapshot,
    });
    // A confirm with the wrong nonce for the pending arm is dropped too.
    server.publish_audio_event(AudioEvent::ArmApplied {
        context: SessionContext {
            session_epoch: session,
            audio_epoch: test_audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: ArmNonce::from_bytes([0x99; 16]),
    });

    assert!(drain_all(&mut rx_a).is_empty());
    assert!(drain_all(&mut rx_b).is_empty());
}

