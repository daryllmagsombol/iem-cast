//! Lane B control tests: generation assignment, safety races, and installed-revision emission.
//!
//! The control actor is exercised directly here; server-boundary authorization is covered in
//! `server::tests`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::contract::{
    AudioEvent, CatalogSnapshot, Clock, ControlError, DspControl, ListenArm, ListenArmed,
    ListenDisarm, MixAck, MixPatch, MixSnapshot, SourceGain, SourceGainMatrix, SourceInfo,
    SourceRole,
};
use crate::control::ControlActor;
use crate::ids::{
    ArmNonce, AudioEpoch, CatalogRevision, HostEpoch, MixRevision, SafetyGeneration, SessionEpoch,
    SourceId,
};
use crate::server::protocol::ServerMessage;

#[derive(Debug)]
struct TestClock {
    now: Mutex<Instant>,
}

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(Instant::now()),
        })
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.now.lock().expect("clock")
    }
}

#[derive(Default)]
struct RecordingDsp {
    disarms: Mutex<Vec<(SessionEpoch, SafetyGeneration)>>,
}

impl DspControl for RecordingDsp {
    fn install_snapshot(&self, _snapshot: MixSnapshot) -> Result<(), ControlError> {
        Ok(())
    }

    fn request_arm(&self, _arm: ListenArm) -> Result<(), ControlError> {
        Ok(())
    }

    fn disarm(&self, session: SessionEpoch, new_generation: SafetyGeneration) {
        self.disarms
            .lock()
            .expect("dsp")
            .push((session, new_generation));
    }
}

fn shared_source() -> SourceId {
    SourceId::from_bytes([0x33; 16])
}

fn unavailable_source() -> SourceId {
    SourceId::from_bytes([0x99; 16])
}

fn test_catalog() -> CatalogSnapshot {
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

fn sess() -> SessionEpoch {
    SessionEpoch::from_bytes([0x22; 16])
}

fn session_b() -> SessionEpoch {
    SessionEpoch::from_bytes([0x23; 16])
}

fn audio_epoch() -> AudioEpoch {
    AudioEpoch::from_bytes([0x55; 16])
}

fn old_nonce() -> ArmNonce {
    ArmNonce::from_bytes([0x66; 16])
}

fn ctx_a() -> crate::contract::SessionContext {
    crate::contract::SessionContext {
        session_epoch: sess(),
        audio_epoch: audio_epoch(),
        safety_generation: SafetyGeneration(0),
    }
}

fn control_actor() -> ControlActor {
    ControlActor::new(
        TestClock::new(),
        Arc::new(RecordingDsp::default()),
        test_catalog(),
        HostEpoch::from_bytes([0x11; 16]),
        audio_epoch(),
    )
}

fn patch_gain_only() -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: shared_source(),
            gain_db: -3.0,
            muted: true,
        }]),
        master_db: -6.0,
        master_muted: true,
    }
}

fn patch_unmute_channel() -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: shared_source(),
            gain_db: -3.0,
            muted: false,
        }]),
        master_db: -6.0,
        master_muted: true,
    }
}

fn patch_mute_channel() -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: shared_source(),
            gain_db: -12.0,
            muted: true,
        }]),
        master_db: -6.0,
        master_muted: true,
    }
}

fn listen_arm_with_gen(generation: u64) -> ListenArm {
    ListenArm {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: audio_epoch(),
            safety_generation: SafetyGeneration(generation),
        },
        applied_revision: MixRevision(0),
        arm_nonce: old_nonce(),
    }
}

fn listen_arm_with_nonce(nonce: ArmNonce) -> ListenArm {
    ListenArm {
        context: ctx_a(),
        applied_revision: MixRevision(0),
        arm_nonce: nonce,
    }
}

fn listen_disarm(generation: SafetyGeneration) -> ListenDisarm {
    ListenDisarm {
        session_epoch: sess(),
        safety_generation: generation,
    }
}

// ---------------------------------------------------------------------------------------------
// Safety gate tests
// ---------------------------------------------------------------------------------------------

#[test]
fn unmute_is_forbidden_while_unarmed_but_gain_is_allowed() {
    let mut a = control_actor();
    assert_eq!(
        a.apply_patch(sess(), patch_unmute_channel()),
        Err(ControlError::UnarmedUnmuteForbidden)
    );
    assert!(a.apply_patch(sess(), patch_gain_only()).is_ok());
}

#[test]
fn mute_is_always_allowed_even_when_unarmed() {
    let mut a = control_actor();
    assert!(a.apply_patch(sess(), patch_mute_channel()).is_ok());
}

#[test]
fn stale_generation_disarm_for_authenticated_current_session_is_applied() {
    let mut a = control_actor();
    let _ = a.apply_patch(sess(), patch_gain_only());
    let _ = a.arm(sess(), listen_arm_with_gen(0));
    assert_eq!(a.current_generation(sess()), SafetyGeneration(1)); // arm assigned gen 1

    let stale = listen_disarm(SafetyGeneration(0)); // stale gen 0
    assert!(a.disarm(sess(), stale).is_ok()); // still accepted for current session
    assert_eq!(a.current_generation(sess()), SafetyGeneration(2)); // disarm bumped to 2
}

#[test]
fn disarm_for_a_foreign_session_is_unauthorized() {
    let mut a = control_actor();
    let foreign = ListenDisarm {
        session_epoch: session_b(),
        safety_generation: SafetyGeneration(0),
    };
    assert_eq!(a.disarm(sess(), foreign), Err(ControlError::Unauthorized));
}

#[test]
fn stale_worker_cannot_arm_after_disarm() {
    let mut a = control_actor();
    let _ = a.arm(sess(), listen_arm_with_gen(0));
    let generation = a.current_generation(sess());
    let _ = a.disarm(sess(), listen_disarm(generation));
    let stale_arm = AudioEvent::ArmApplied {
        context: ctx_a(),
        arm_nonce: old_nonce(),
    };
    assert!(a.on_audio_event(stale_arm).is_none());
}

#[test]
fn arm_confirmation_requires_the_exact_pending_tuple() {
    let mut a = control_actor();
    let _ = a.arm(sess(), listen_arm_with_gen(0));
    let wrong_nonce = AudioEvent::ArmApplied {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: ArmNonce::from_bytes([0x77; 16]),
    };
    assert!(a.on_audio_event(wrong_nonce).is_none());

    let confirmed = AudioEvent::ArmApplied {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: old_nonce(),
    };
    assert_eq!(
        a.on_audio_event(confirmed),
        Some(ServerMessage::ListenArmed(ListenArmed {
            session_epoch: sess(),
            safety_generation: SafetyGeneration(1),
            audio_epoch: audio_epoch(),
            arm_nonce: old_nonce(),
        }))
    );
}

#[test]
fn cancelled_arm_nonce_is_permanently_invalid_for_that_attempt() {
    let mut a = control_actor();
    let n = ArmNonce::from_bytes([0x05; 16]);
    let _ = a.arm(sess(), listen_arm_with_nonce(n));
    let generation = a.current_generation(sess());
    let _ = a.disarm(sess(), listen_disarm(generation));
    let replay = AudioEvent::ArmApplied {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: audio_epoch(),
            safety_generation: generation,
        },
        arm_nonce: n,
    };
    assert!(a.on_audio_event(replay).is_none());
}

#[test]
fn reusing_a_consumed_arm_nonce_is_rejected() {
    let mut a = control_actor();
    let n = ArmNonce::from_bytes([0x07; 16]);
    assert!(a.arm(sess(), listen_arm_with_nonce(n)).is_ok());
    assert_eq!(
        a.arm(sess(), listen_arm_with_nonce(n)),
        Err(ControlError::StaleEpoch)
    );
}

// ---------------------------------------------------------------------------------------------
// Installed-revision emission
// ---------------------------------------------------------------------------------------------

#[test]
fn mix_applied_is_emitted_only_for_installed_revision() {
    let mut a = control_actor();
    let ack: MixAck = a.apply_patch(sess(), patch_gain_only()).unwrap();

    let installed = AudioEvent::MixApplied {
        applied_revision: ack.accepted_revision,
        context: ack.canonical_snapshot.context,
        start_sample: 0,
        snapshot: ack.canonical_snapshot,
    };
    assert!(matches!(
        a.on_audio_event(installed),
        Some(ServerMessage::MixApplied(_))
    ));

    let skipped = AudioEvent::MixApplied {
        applied_revision: MixRevision(9999),
        context: ctx_a(),
        start_sample: 0,
        snapshot: ack.canonical_snapshot,
    };
    assert!(a.on_audio_event(skipped).is_none());
}

#[test]
fn stale_generation_mix_applied_is_never_emitted() {
    let mut a = control_actor();
    let ack = a.apply_patch(sess(), patch_gain_only()).unwrap();
    // A different safety generation than the actor currently holds must be dropped.
    let stale_context = crate::contract::SessionContext {
        session_epoch: sess(),
        audio_epoch: audio_epoch(),
        safety_generation: SafetyGeneration(5),
    };
    let event = AudioEvent::MixApplied {
        applied_revision: ack.accepted_revision,
        context: stale_context,
        start_sample: 0,
        snapshot: ack.canonical_snapshot,
    };
    assert!(a.on_audio_event(event).is_none());
}

#[test]
fn stale_base_revision_is_a_conflict() {
    let mut a = control_actor();
    let _ = a.apply_patch(sess(), patch_gain_only()).unwrap();
    // The second patch still claims base revision 0.
    assert_eq!(
        a.apply_patch(sess(), patch_gain_only()),
        Err(ControlError::RevisionConflict)
    );
}

#[test]
fn unavailable_source_is_forbidden() {
    let mut a = control_actor();
    let patch = MixPatch {
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: unavailable_source(),
            gain_db: -3.0,
            muted: true,
        }]),
        ..patch_gain_only()
    };
    assert_eq!(
        a.apply_patch(sess(), patch),
        Err(ControlError::SourceForbidden)
    );
}

#[test]
fn nonfinite_gain_is_rejected() {
    let mut a = control_actor();
    let patch = MixPatch {
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: shared_source(),
            gain_db: f32::NAN,
            muted: true,
        }]),
        ..patch_gain_only()
    };
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
}

#[test]
fn arm_with_stale_audio_epoch_is_rejected() {
    let mut a = control_actor();
    let arm = ListenArm {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: AudioEpoch::from_bytes([0x99; 16]),
            safety_generation: SafetyGeneration(0),
        },
        applied_revision: MixRevision(0),
        arm_nonce: old_nonce(),
    };
    assert_eq!(a.arm(sess(), arm), Err(ControlError::StaleEpoch));
}

#[test]
fn rate_limit_rejects_a_burst_beyond_capacity() {
    let mut a = control_actor();
    // The burst allowance is 60; 61 rapid attempts must hit the limiter.
    let mut limited = false;
    for _ in 0..61 {
        if matches!(
            a.apply_patch(sess(), patch_gain_only()),
            Err(ControlError::RateLimited)
        ) {
            limited = true;
            break;
        }
    }
    assert!(limited, "burst beyond capacity must be rate limited");
}

#[test]
fn idempotent_ack_cache_replays_without_advancing_revision() {
    let mut a = control_actor();
    let ack = a.apply_patch(sess(), patch_gain_only()).unwrap();
    a.remember_ack(sess(), "req-1", ack);
    assert_eq!(a.cached_ack(sess(), "req-1"), Some(ack));
    // A different request id is not cached.
    assert_eq!(a.cached_ack(sess(), "req-2"), None);
}

#[test]
fn duration_helper_is_used() {
    // Keep the Duration import meaningful for the clock helper.
    let _ = Duration::from_secs(0);
}
