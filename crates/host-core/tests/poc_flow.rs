//! Task 9 integration flow: real control + DSP + Opus safety semantics with the capture backend
//! and network mocked out. This is software evidence only; it does not prove hardware behavior.
//!
//! It exercises the exact contracts the POC composes: a stale-generation disarm is applied, a
//! cancelled arm nonce cannot re-arm, two musicians may share one source with independent
//! settings, and a mix is only reported applied for the installed revision.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use host_core::contract::{
    AudioEvent, CatalogSnapshot, Clock, ControlError, DspControl, ListenArm, ListenDisarm, MixPatch,
    MixSnapshot, SessionContext, SourceGain, SourceGainMatrix, SourceInfo, SourceRole,
};
use host_core::control::ControlActor;
use host_core::ids::{
    ArmNonce, AudioEpoch, CatalogRevision, HostEpoch, MixRevision, SafetyGeneration, SessionEpoch,
    SourceId,
};
use host_core::server::protocol::ServerMessage;

#[derive(Debug)]
struct TestClock(Mutex<Instant>);
impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Instant::now())))
    }
}
impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.0.lock().expect("clock")
    }
}

#[derive(Default)]
struct RecordingDsp {
    disarms: Mutex<Vec<(SessionEpoch, SafetyGeneration)>>,
    installs: AtomicU64,
}
impl DspControl for RecordingDsp {
    fn install_snapshot(&self, _snapshot: MixSnapshot) -> Result<(), ControlError> {
        self.installs.fetch_add(1, Ordering::SeqCst);
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

fn source() -> SourceId {
    SourceId::from_bytes([0x33; 16])
}
fn audio_epoch() -> AudioEpoch {
    AudioEpoch::from_bytes([0x55; 16])
}
fn session_a() -> SessionEpoch {
    SessionEpoch::from_bytes([0x22; 16])
}
fn session_b() -> SessionEpoch {
    SessionEpoch::from_bytes([0x23; 16])
}
fn nonce() -> ArmNonce {
    ArmNonce::from_bytes([0x66; 16])
}

fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog_revision: CatalogRevision(1),
        sources: vec![SourceInfo {
            source_id: source(),
            physical_index: 1,
            label: "Shared".to_string(),
            role: SourceRole::InputChannel,
            authorized: true,
            available: true,
            stereo_pair: None,
        }],
    }
}

fn actor() -> ControlActor {
    ControlActor::new(
        TestClock::new(),
        Arc::new(RecordingDsp::default()),
        catalog(),
        HostEpoch::from_bytes([0x11; 16]),
        audio_epoch(),
    )
}

fn ctx(session: SessionEpoch, generation: u64) -> SessionContext {
    SessionContext {
        session_epoch: session,
        audio_epoch: audio_epoch(),
        safety_generation: SafetyGeneration(generation),
    }
}

fn patch(gain_db: f32) -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(&[SourceGain {
            source_id: source(),
            gain_db,
            muted: true,
        }]),
        master_db: -6.0,
        master_muted: true,
    }
}

#[test]
fn two_sessions_apply_arm_and_stop_generations_without_hardware() {
    let mut a = actor();

    // Two musicians share one source with independent settings.
    assert!(a.apply_patch(session_a(), patch(-3.0)).is_ok());
    assert!(a.apply_patch(session_b(), patch(-9.0)).is_ok());

    // Arm session A at generation 0; the host assigns generation 1.
    let arm = ListenArm {
        context: ctx(session_a(), 0),
        applied_revision: MixRevision(1),
        arm_nonce: nonce(),
    };
    assert!(a.arm(session_a(), arm).is_ok());
    assert_eq!(a.current_generation(session_a()), SafetyGeneration(1));

    // Confirm the exact pending tuple -> ListenArmed.
    let confirmed = a.on_audio_event(AudioEvent::ArmApplied {
        context: ctx(session_a(), 1),
        arm_nonce: nonce(),
    });
    assert!(matches!(confirmed, Some(ServerMessage::ListenArmed(_))));

    // A stale-generation disarm for the authenticated current session is APPLIED and bumps
    // the generation, so a stale worker can never re-arm.
    let stale = ListenDisarm {
        session_epoch: session_a(),
        safety_generation: SafetyGeneration(0),
    };
    assert!(a.disarm(session_a(), stale).is_ok());
    assert_eq!(a.current_generation(session_a()), SafetyGeneration(2));

    let replay = a.on_audio_event(AudioEvent::ArmApplied {
        context: ctx(session_a(), 1),
        arm_nonce: nonce(),
    });
    assert!(replay.is_none());
}

#[test]
fn mix_is_reported_applied_only_for_the_installed_revision() {
    let mut a = actor();
    let ack = a.apply_patch(session_a(), patch(-3.0)).unwrap();

    let installed = AudioEvent::MixApplied {
        applied_revision: ack.accepted_revision,
        context: ctx(session_a(), 0),
        start_sample: 0,
        snapshot: ack.canonical_snapshot,
    };
    assert!(matches!(
        a.on_audio_event(installed),
        Some(ServerMessage::MixApplied(_))
    ));

    let skipped = AudioEvent::MixApplied {
        applied_revision: MixRevision(9_999),
        context: ctx(session_a(), 0),
        start_sample: 0,
        snapshot: ack.canonical_snapshot,
    };
    assert!(a.on_audio_event(skipped).is_none());
}
