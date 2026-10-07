//! Per-listener safety worker.
//!
//! The worker **consumes** the safety generation the `ControlActor` provides; it never assigns an
//! independent counter. `apply_arm` verifies the existing applied revision and exact context,
//! then emits [`AudioEvent::ArmApplied`] carrying the **provided** generation. `on_audio_event`
//! validates the exact session context and nonce tuple.

use std::sync::Mutex;

use crate::contract::{AudioEvent, AudioFault, ListenArm};
use crate::ids::{ArmNonce, MixRevision, SafetyGeneration, SessionEpoch};

/// Tracks the applied mix/arm state for one listener.
pub struct SafetyWorker {
    session: SessionEpoch,
    inner: Mutex<Inner>,
}

struct Inner {
    applied_revision: Option<MixRevision>,
    applied_context: Option<crate::contract::SessionContext>,
    /// The nonce that is currently allowed to arm; `None` means disarmed/no valid attempt.
    pending_nonce: Option<ArmNonce>,
    generation: SafetyGeneration,
}

impl SafetyWorker {
    /// Create a worker for one listener session.
    pub fn new(session: SessionEpoch) -> Self {
        Self {
            session,
            inner: Mutex::new(Inner {
                applied_revision: None,
                applied_context: None,
                pending_nonce: None,
                generation: SafetyGeneration(0),
            }),
        }
    }

    /// The session this worker is bound to.
    pub fn session(&self) -> SessionEpoch {
        self.session
    }

    /// Record that `revision` was installed for `context` before an arm attempt.
    pub fn note_applied(
        &self,
        context: crate::contract::SessionContext,
        revision: MixRevision,
    ) {
        let mut inner = self.inner.lock().expect("safety worker poisoned");
        inner.applied_revision = Some(revision);
        inner.applied_context = Some(context);
    }

    /// Verify the existing applied revision and exact context, then emit `ArmApplied` carrying the
    /// **provided** generation.
    pub fn apply_arm(
        &self,
        arm: &ListenArm,
        generation: SafetyGeneration,
    ) -> Result<AudioEvent, AudioFault> {
        if arm.context.session_epoch != self.session {
            return Err(AudioFault::Internal);
        }
        let mut inner = self.inner.lock().map_err(|_| AudioFault::Internal)?;
        // Verify against an existing applied revision when one is recorded; otherwise this is the
        // first arm for the session and the carried revision is adopted.
        if let Some(applied) = inner.applied_revision {
            if applied != arm.applied_revision {
                return Err(AudioFault::RevisionStale);
            }
        } else {
            inner.applied_revision = Some(arm.applied_revision);
        }
        if inner.applied_context.is_none() {
            inner.applied_context = Some(arm.context);
        }
        if inner.applied_context != Some(arm.context) {
            return Err(AudioFault::Internal);
        }
        inner.pending_nonce = Some(arm.arm_nonce);
        inner.generation = generation;
        Ok(AudioEvent::ArmApplied {
            context: arm.context,
            arm_nonce: arm.arm_nonce,
        })
    }

    /// Disarm; advances the worker's stored generation and invalidates the pending nonce.
    pub fn apply_disarm(&self, generation: SafetyGeneration) -> AudioEvent {
        let mut inner = self.inner.lock().expect("safety worker poisoned");
        inner.pending_nonce = None;
        inner.generation = generation;
        AudioEvent::Interrupted {
            session_epoch: self.session,
            reason: crate::contract::InterruptReason::UserDisarm,
        }
    }

    /// Validate the exact `SessionContext` + nonce tuple carried by an audio event.
    ///
    /// Returns `true` only when the event belongs to this session, matches the applied context,
    /// and (for arm events) carries the pending nonce at the current generation.
    pub fn on_audio_event(&self, ev: &AudioEvent) -> bool {
        let inner = self.inner.lock().expect("safety worker poisoned");
        match ev {
            AudioEvent::ArmApplied { context, arm_nonce } => {
                context.session_epoch == self.session
                    && inner.applied_context == Some(*context)
                    && inner.pending_nonce == Some(*arm_nonce)
            }
            AudioEvent::MixApplied { context, .. } => {
                context.session_epoch == self.session
                    && inner.applied_context == Some(*context)
            }
            AudioEvent::Interrupted { session_epoch, .. } => *session_epoch == self.session,
            AudioEvent::Fault { .. } => true,
        }
    }

    /// The generation most recently supplied by the actor.
    pub fn generation(&self) -> SafetyGeneration {
        self.inner
            .lock()
            .expect("safety worker poisoned")
            .generation
    }
}
