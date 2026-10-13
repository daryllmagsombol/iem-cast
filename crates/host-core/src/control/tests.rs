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
    installs: Mutex<Vec<MixSnapshot>>,
}

impl DspControl for RecordingDsp {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError> {
        self.installs.lock().expect("dsp").push(snapshot);
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
                stereo_side: None,
                default_pan: 0.0,
            },
            SourceInfo {
                source_id: unavailable_source(),
                physical_index: 2,
                label: "Offline".to_string(),
                role: SourceRole::InputChannel,
                authorized: true,
                available: false,
                stereo_pair: None,
                stereo_side: None,
                default_pan: 0.0,
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
            pan: 0.0,
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
            pan: 0.0,
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
            pan: 0.0,
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
            pan: 0.0,
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
            pan: 0.0,
        }]),
        ..patch_gain_only()
    };
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
}

#[test]
fn canonicalize_pan_rejects_nonfinite_and_clamps_finite() {
    use crate::control::revisions::canonicalize_pan;

    assert_eq!(canonicalize_pan(f32::NAN), Err(ControlError::Internal));
    assert_eq!(canonicalize_pan(f32::INFINITY), Err(ControlError::Internal));
    assert_eq!(canonicalize_pan(f32::NEG_INFINITY), Err(ControlError::Internal));
    assert_eq!(canonicalize_pan(-2.0), Ok(-1.0));
    assert_eq!(canonicalize_pan(5.0), Ok(1.0));
    assert_eq!(canonicalize_pan(0.35), Ok(0.35));
}

#[test]
fn silent_source_gain_is_center_and_muted() {
    assert_eq!(
        SourceGain::SILENT,
        SourceGain {
            source_id: SourceId::from_bytes([0; 16]),
            gain_db: 0.0,
            muted: true,
            pan: 0.0,
        }
    );
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

// ---------------------------------------------------------------------------------------------
// Linked stereo pair validation (Task 2)
//
// A linked pair is a single atomic edit: both members present, equal canonical pan, both
// authorized+available together. An invalid/incomplete pair is a rejection at the control
// boundary, never a silently degraded mono source. These use their own explicit pair catalog so
// the existing single-source fixtures are untouched.
// ---------------------------------------------------------------------------------------------

fn pair_left() -> SourceId {
    SourceId::from_bytes([0x41; 16])
}

fn pair_right() -> SourceId {
    SourceId::from_bytes([0x42; 16])
}

fn pair_source_info(
    source_id: SourceId,
    physical_index: u16,
    peer: SourceId,
    side: crate::contract::StereoSide,
    available: bool,
) -> SourceInfo {
    SourceInfo {
        source_id,
        physical_index,
        label: format!("{source_id}"),
        role: SourceRole::InputChannel,
        authorized: true,
        available,
        stereo_pair: Some(peer),
        stereo_side: Some(side),
        default_pan: 0.0,
    }
}

/// A catalog whose only pair is `pair_left`/`pair_right` (reciprocal, opposite sides). `left_avail`
/// and `right_avail` let a test make one side unavailable.
fn pair_catalog_with(left_avail: bool, right_avail: bool) -> CatalogSnapshot {
    use crate::contract::StereoSide;
    CatalogSnapshot {
        catalog_revision: CatalogRevision(1),
        sources: vec![
            pair_source_info(pair_left(), 3, pair_right(), StereoSide::Left, left_avail),
            pair_source_info(pair_right(), 1, pair_left(), StereoSide::Right, right_avail),
        ],
    }
}

fn pair_actor_with(left_avail: bool, right_avail: bool) -> ControlActor {
    ControlActor::new(
        TestClock::new(),
        Arc::new(RecordingDsp::default()),
        pair_catalog_with(left_avail, right_avail),
        HostEpoch::from_bytes([0x11; 16]),
        audio_epoch(),
    )
}

/// A pair actor plus a handle to its recording DSP, so installs can be counted.
fn pair_actor_and_dsp(left_avail: bool, right_avail: bool) -> (ControlActor, Arc<RecordingDsp>) {
    let dsp = Arc::new(RecordingDsp::default());
    let actor = ControlActor::new(
        TestClock::new(),
        dsp.clone(),
        pair_catalog_with(left_avail, right_avail),
        HostEpoch::from_bytes([0x11; 16]),
        audio_epoch(),
    );
    (actor, dsp)
}

fn pair_gain(source_id: SourceId, gain_db: f32, pan: f32) -> SourceGain {
    SourceGain {
        source_id,
        gain_db,
        muted: true,
        pan,
    }
}

fn pair_patch(sources: &[SourceGain]) -> MixPatch {
    MixPatch {
        base_revision: MixRevision(0),
        catalog_revision: CatalogRevision(1),
        sources: SourceGainMatrix::from_slice(sources),
        master_db: -6.0,
        master_muted: true,
    }
}

#[test]
fn linked_pair_unequal_pan_is_rejected() {
    let mut a = pair_actor_with(true, true);
    let patch = pair_patch(&[
        pair_gain(pair_left(), -3.0, 0.5),
        pair_gain(pair_right(), -6.0, -0.5),
    ]);
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
}

#[test]
fn linked_pair_equal_pan_with_independent_gain_and_mute_is_accepted() {
    let mut a = pair_actor_with(true, true);
    // Accept a fully-muted pair first (mute is always allowed while unarmed).
    let both_muted = pair_patch(&[
        pair_gain(pair_left(), -3.0, -0.25),
        pair_gain(pair_right(), -12.0, -0.25),
    ]);
    assert!(a.apply_patch(sess(), both_muted).is_ok(), "equal-pan pair with independent gains must pass");

    // Arm so unmuting one member is permitted, then edit only that member's mute/gain.
    let arm = listen_arm_with_gen(0);
    assert!(a.arm(sess(), arm).is_ok());
    // Confirm the pending arm so `armed` becomes true (the DSP ack path).
    let confirmed = AudioEvent::ArmApplied {
        context: crate::contract::SessionContext {
            session_epoch: sess(),
            audio_epoch: audio_epoch(),
            safety_generation: SafetyGeneration(1),
        },
        arm_nonce: arm.arm_nonce,
    };
    assert!(a.on_audio_event(confirmed).is_some());
    let mut left = pair_gain(pair_left(), -6.0, -0.25);
    left.muted = false; // independent mute: left unmuted, right stays muted
    let edited = MixPatch {
        base_revision: MixRevision(1),
        ..pair_patch(&[left, pair_gain(pair_right(), -12.0, -0.25)])
    };
    assert!(a.apply_patch(sess(), edited).is_ok(), "independent mute on a linked pair must be allowed");
}

#[test]
fn linked_pair_pan_equality_compares_canonicalized_values() {
    // 1.5 and 2.0 both canonicalize to 1.0, so they are equal; -0.5 differs.
    let mut a = pair_actor_with(true, true);
    let accepted = pair_patch(&[
        pair_gain(pair_left(), -3.0, 1.5),
        pair_gain(pair_right(), -3.0, 2.0),
    ]);
    assert!(a.apply_patch(sess(), accepted).is_ok(), "pan must be clamped before comparison");

    let mut b = pair_actor_with(true, true);
    let rejected = pair_patch(&[
        pair_gain(pair_left(), -3.0, 1.5),
        pair_gain(pair_right(), -3.0, -0.5),
    ]);
    assert_eq!(b.apply_patch(sess(), rejected), Err(ControlError::Internal));
}

#[test]
fn linked_pair_pan_equality_is_exact_not_tolerant() {
    // A differing pair pan smaller than f32::EPSILON must still be rejected: the accepted pair state
    // must carry one exact canonical value, not a tolerance-window match.
    let mut a = pair_actor_with(true, true);
    let patch = pair_patch(&[
        pair_gain(pair_left(), -3.0, 0.0),
        pair_gain(pair_right(), -3.0, f32::EPSILON),
    ]);
    assert_eq!(
        a.apply_patch(sess(), patch),
        Err(ControlError::Internal),
        "differing canonical pans (0.0 vs EPSILON) must be rejected exactly"
    );

    // After out-of-range clamping the two pans are bit-identical (both 1.0) and must be accepted;
    // gains and mutes stay independent.
    let mut b = pair_actor_with(true, true);
    let clamped = pair_patch(&[
        pair_gain(pair_left(), -3.0, 5.0),
        pair_gain(pair_right(), -12.0, 2.5),
    ]);
    let ack = b.apply_patch(sess(), clamped).expect("clamped pans are exactly equal");
    assert_eq!(ack.accepted_revision, MixRevision(1));
}

#[test]
fn linked_pair_member_absent_is_rejected_half_pair() {
    let mut a = pair_actor_with(true, true);
    // Only the left member is present.
    let patch = pair_patch(&[pair_gain(pair_left(), -3.0, 0.0)]);
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
}

#[test]
fn unavailable_pair_member_is_fail_closed_together() {
    // Right side unavailable: the whole pair is rejected, never degraded to a mono source.
    let mut a = pair_actor_with(true, false);
    let patch = pair_patch(&[
        pair_gain(pair_left(), -3.0, 0.0),
        pair_gain(pair_right(), -3.0, 0.0),
    ]);
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::SourceForbidden));
}

#[test]
fn duplicate_source_id_in_patch_is_rejected() {
    let mut a = control_actor();
    let patch = MixPatch {
        sources: SourceGainMatrix::from_slice(&[
            pair_gain(shared_source(), -3.0, 0.0),
            pair_gain(shared_source(), -9.0, 0.0),
        ]),
        ..patch_gain_only()
    };
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
}

#[test]
fn stale_catalog_revision_is_rejected() {
    let mut a = control_actor();
    let patch = MixPatch {
        catalog_revision: CatalogRevision(2), // actor catalog is revision 1
        ..patch_gain_only()
    };
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::RevisionConflict));
}

#[test]
fn rejected_pair_does_not_advance_accepted_revision_or_cache() {
    let (mut a, dsp) = pair_actor_and_dsp(true, true);
    let patch = pair_patch(&[
        pair_gain(pair_left(), -3.0, 0.5),
        pair_gain(pair_right(), -6.0, -0.5),
    ]);
    assert_eq!(a.apply_patch(sess(), patch), Err(ControlError::Internal));
    // The rejected edit must not have changed accepted state: a valid patch still claims base 0.
    let ok = pair_patch(&[
        pair_gain(pair_left(), -3.0, 0.0),
        pair_gain(pair_right(), -3.0, 0.0),
    ]);
    let ack = a.apply_patch(sess(), ok).expect("base revision must remain 0 after a rejection");
    assert_eq!(ack.accepted_revision, MixRevision(1));
    assert_eq!(dsp.installs.lock().unwrap().len(), 1, "only the accepted patch installs");
}

#[test]
fn rejected_pair_preserves_seeded_accepted_snapshot_revision_and_ack_cache() {
    // Seed real accepted state and an acknowledgement, then reject an unequal-pan pair, then verify
    // the accepted snapshot, revision, DSP install count, and idempotent ack are all untouched.
    let (mut a, dsp) = pair_actor_and_dsp(true, true);

    let seed = pair_patch(&[
        pair_gain(pair_left(), -3.0, -0.25),
        pair_gain(pair_right(), -6.0, -0.25),
    ]);
    let seed_ack = a.apply_patch(sess(), seed).expect("seed accepted pair");
    assert_eq!(seed_ack.accepted_revision, MixRevision(1));
    a.remember_ack(sess(), "req-1", seed_ack);

    let rejected = MixPatch {
        base_revision: MixRevision(1),
        ..pair_patch(&[
            pair_gain(pair_left(), -3.0, 0.5),
            pair_gain(pair_right(), -6.0, -0.5),
        ])
    };
    // A rejection may still consume the pre-existing rate-limit bookkeeping token; it must not
    // change accepted state. Use the same session so bookkeeping is observable across calls.
    assert_eq!(a.apply_patch(sess(), rejected), Err(ControlError::Internal));

    // Accepted snapshot and revision are unchanged.
    let accepted = a.accepted_snapshot(sess()).expect("accepted snapshot present");
    assert_eq!(accepted.mix_revision, MixRevision(1));
    assert_eq!(accepted.sources.as_slice()[0].pan, -0.25);
    assert_eq!(accepted.sources.as_slice()[1].pan, -0.25);
    assert_eq!(accepted.sources.as_slice()[0].gain_db, -3.0);
    assert_eq!(accepted.sources.as_slice()[1].gain_db, -6.0);
    // The DSP was only asked to install the seed; the rejection never reached it.
    assert_eq!(dsp.installs.lock().unwrap().len(), 1, "rejection must not install to the DSP");
    // The earlier acknowledgement is still cached verbatim, and a retry of the rejected edit using
    // the same base revision is still a rejection (revision did not advance).
    assert_eq!(a.cached_ack(sess(), "req-1"), Some(seed_ack));
    let retry = MixPatch {
        base_revision: MixRevision(1),
        ..pair_patch(&[
            pair_gain(pair_left(), -3.0, 0.5),
            pair_gain(pair_right(), -6.0, -0.5),
        ])
    };
    assert_eq!(a.apply_patch(sess(), retry), Err(ControlError::Internal));
    assert_eq!(dsp.installs.lock().unwrap().len(), 1);

    // A *valid* follow-up with the base revision the seed left behind (1) must be accepted as
    // revision 2. This proves the rejected edits (which fail at pair validation, before revision
    // validation) did not corrupt the accepted counter, rather than only that a same-base retry
    // still fails.
    let valid = MixPatch {
        base_revision: MixRevision(1),
        ..pair_patch(&[
            pair_gain(pair_left(), -3.0, 0.25),
            pair_gain(pair_right(), -6.0, 0.25),
        ])
    };
    let ack2 = a
        .apply_patch(sess(), valid)
        .expect("a valid follow-up at base revision 1 must be accepted");
    assert_eq!(ack2.accepted_revision, MixRevision(2));
    assert_eq!(dsp.installs.lock().unwrap().len(), 2, "the valid follow-up installs once");
}
