//! Authoritative control actor.
//!
//! The actor owns per-listener safety generations, accepted/installed mix revisions, and the
//! pairing store. It is the only component allowed to assign a [`SafetyGeneration`]; the DSP
//! safety worker consumes the generation it is handed. Authentication itself is enforced at the
//! server boundary (`cookie` + envelope), so every method here receives an already-authenticated
//! [`SessionEpoch`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::contract::{
    AudioEvent, CatalogSnapshot, Clock, ControlError, DspControl, ListenArm, ListenArmed,
    ListenDisarm, MixAck, MixApplied, MixPatch, MixSnapshot, SessionContext, SourceGain,
    SourceGainMatrix,
};
use crate::ids::{
    ArmNonce, AudioEpoch, HostEpoch, MixRevision, SafetyGeneration, SessionEpoch, SourceId,
};
use crate::server::pairing::{PairStore, PairingCredential};
use crate::server::protocol::ServerMessage;

use super::revisions::{canonicalize_gain, RateLimiter, PATCH_BURST, PATCH_RATE_PER_SEC};

/// Bounded number of accepted-patch acknowledgements retained for idempotency.
const REQUEST_CACHE_LIMIT: usize = 64;
/// Bounded arm-nonce history retained to keep cancelled attempts permanently invalid.
const NONCE_HISTORY_LIMIT: usize = 32;

#[derive(Debug)]
struct SessionState {
    generation: SafetyGeneration,
    accepted_revision: MixRevision,
    installed_revision: MixRevision,
    accepted_snapshot: Option<MixSnapshot>,
    applied_snapshot: Option<MixSnapshot>,
    previous_muted: HashMap<SourceId, bool>,
    previous_master_muted: bool,
    pending_arm: Option<(ArmNonce, SessionContext)>,
    used_nonces: VecDeque<ArmNonce>,
    cancelled_nonces: HashSet<ArmNonce>,
    armed: bool,
    limiter: RateLimiter,
    cache: VecDeque<(String, MixAck)>,
}

impl SessionState {
    fn new() -> Self {
        Self {
            generation: SafetyGeneration(0),
            accepted_revision: MixRevision(0),
            installed_revision: MixRevision(0),
            accepted_snapshot: None,
            applied_snapshot: None,
            previous_muted: HashMap::new(),
            previous_master_muted: true,
            pending_arm: None,
            used_nonces: VecDeque::new(),
            cancelled_nonces: HashSet::new(),
            armed: false,
            limiter: RateLimiter::new(PATCH_RATE_PER_SEC, PATCH_BURST),
            cache: VecDeque::new(),
        }
    }

    fn remember_nonce(&mut self, nonce: ArmNonce) {
        self.used_nonces.push_back(nonce);
        if self.used_nonces.len() > NONCE_HISTORY_LIMIT {
            self.used_nonces.pop_front();
        }
    }

    fn cancel_pending_arm(&mut self) {
        if let Some((nonce, _)) = self.pending_arm.take() {
            self.cancelled_nonces.insert(nonce);
        }
        self.armed = false;
    }
}

/// Authoritative control actor for all listener sessions.
pub struct ControlActor {
    clock: Arc<dyn Clock>,
    dsp: Arc<dyn DspControl>,
    catalog: CatalogSnapshot,
    host_epoch: HostEpoch,
    audio_epoch: AudioEpoch,
    sessions: HashMap<SessionEpoch, SessionState>,
    pair_store: Arc<Mutex<PairStore>>,
}

impl ControlActor {
    /// Construct the actor. The actor mints its own OS-random pairing store.
    pub fn new(
        clock: Arc<dyn Clock>,
        dsp: Arc<dyn DspControl>,
        catalog: CatalogSnapshot,
        host_epoch: HostEpoch,
        audio_epoch: AudioEpoch,
    ) -> Self {
        let entropy: Arc<dyn crate::contract::Entropy> = Arc::new(crate::control::auth::OsEntropy);
        let pair_store = PairStore::new(entropy, clock.clone());
        Self {
            clock,
            dsp,
            catalog,
            host_epoch,
            audio_epoch,
            sessions: HashMap::new(),
            pair_store: Arc::new(Mutex::new(pair_store)),
        }
    }

    /// The global host identity this actor was started with.
    pub fn host_epoch(&self) -> HostEpoch {
        self.host_epoch
    }

    /// The capture generation this actor was started with.
    pub fn audio_epoch(&self) -> AudioEpoch {
        self.audio_epoch
    }

    /// Override the base URL used to build pairing join links (host composition sets this from the
    /// selected interface; tests keep the default).
    pub fn set_join_base(&mut self, base: impl Into<String>) {
        if let Ok(mut store) = self.pair_store.lock() {
            store.set_join_base(base);
        }
    }

    /// The current safety generation for a session (0 for a session the actor has not seen).
    pub fn current_generation(&self, session: SessionEpoch) -> SafetyGeneration {
        self.sessions
            .get(&session)
            .map(|s| s.generation)
            .unwrap_or(SafetyGeneration(0))
    }

    /// The last accepted per-source gain, if any (used by tests and the operator view).
    pub fn accepted_gain(&self, session: SessionEpoch, source: SourceId) -> Option<f32> {
        let snapshot = self.sessions.get(&session)?.accepted_snapshot?;
        snapshot
            .sources
            .as_slice()
            .iter()
            .find(|g| g.source_id == source)
            .map(|g| g.gain_db)
    }

    /// The accepted mix snapshot for a session, if any.
    pub fn accepted_snapshot(&self, session: SessionEpoch) -> Option<MixSnapshot> {
        self.sessions.get(&session)?.accepted_snapshot
    }

    fn state_mut(&mut self, session: SessionEpoch) -> &mut SessionState {
        self.sessions.entry(session).or_insert_with(SessionState::new)
    }

    /// Mint a fresh single-use pairing credential (operator path).
    pub fn issue_pairing_credential(&mut self) -> PairingCredential {
        match self.pair_store.lock() {
            Ok(mut store) => store.issue_credential(),
            Err(poisoned) => poisoned.into_inner().issue_credential(),
        }
    }

    /// Consume a pairing token (server boundary). One use, 120 s expiry.
    pub fn exchange_pairing_credential(&mut self, token: &str) -> Result<(), ControlError> {
        let result = match self.pair_store.lock() {
            Ok(mut store) => store.exchange(token),
            Err(poisoned) => poisoned.into_inner().exchange(token),
        };
        result.map_err(|_| ControlError::Unauthorized)
    }

    fn catalog_source(&self, source: SourceId) -> Option<&crate::contract::SourceInfo> {
        self.catalog.sources.iter().find(|s| s.source_id == source)
    }

    /// Apply an absolute mix patch for an authenticated session.
    ///
    /// Rejects stale base revisions, forbidden sources, and unmutes while unarmed. Mute and gain
    /// changes are always allowed.
    pub fn apply_patch(
        &mut self,
        session: SessionEpoch,
        request: MixPatch,
    ) -> Result<MixAck, ControlError> {
        let now = self.clock.now();
        let catalog_revision = self.catalog.catalog_revision;
        let audio_epoch = self.audio_epoch;

        // Rate limiting is enforced here as well as at the boundary.
        {
            let state = self.state_mut(session);
            if !state.limiter.try_take(now) {
                return Err(ControlError::RateLimited);
            }
        }

        // Resolve and validate entries before mutating any state.
        let mut canonical: Vec<SourceGain> = Vec::with_capacity(request.sources.len as usize);
        let mut unmute_forbidden = false;
        for entry in request.sources.as_slice() {
            let info = self
                .catalog_source(entry.source_id)
                .ok_or(ControlError::SourceForbidden)?;
            if !info.authorized || !info.available {
                return Err(ControlError::SourceForbidden);
            }
            let gain_db = canonicalize_gain(entry.gain_db)?;
            canonical.push(SourceGain {
                source_id: entry.source_id,
                gain_db,
                muted: entry.muted,
            });
        }
        let master_db = canonicalize_gain(request.master_db)?;

        // Validate and build the snapshot under a scoped borrow, then release it before the DSP
        // call so the actor can also mutate DSP-adjacent state afterwards.
        let (accepted_revision, snapshot) = {
            let state = self.state_mut(session);
            if request.base_revision != state.accepted_revision {
                return Err(ControlError::RevisionConflict);
            }

            for entry in &canonical {
                let previously_muted = state
                    .previous_muted
                    .get(&entry.source_id)
                    .copied()
                    .unwrap_or(true);
                if !entry.muted && previously_muted && !state.armed {
                    unmute_forbidden = true;
                }
            }
            if !request.master_muted && state.previous_master_muted && !state.armed {
                unmute_forbidden = true;
            }
            if unmute_forbidden {
                return Err(ControlError::UnarmedUnmuteForbidden);
            }

            let accepted_revision = MixRevision(state.accepted_revision.0 + 1);
            let snapshot = MixSnapshot {
                context: SessionContext {
                    session_epoch: session,
                    audio_epoch,
                    safety_generation: state.generation,
                },
                catalog_revision,
                mix_revision: accepted_revision,
                sources: SourceGainMatrix::from_slice(&canonical),
                master_db,
                master_muted: request.master_muted,
            };
            (accepted_revision, snapshot)
        };

        // Install before committing; a DSP rejection must not advance the revision.
        self.dsp.install_snapshot(snapshot)?;

        let state = self.state_mut(session);
        for entry in &canonical {
            state.previous_muted.insert(entry.source_id, entry.muted);
        }
        state.previous_master_muted = request.master_muted;
        state.accepted_revision = accepted_revision;
        state.installed_revision = accepted_revision;
        state.accepted_snapshot = Some(snapshot);

        Ok(MixAck {
            accepted_revision,
            catalog_revision,
            canonical_snapshot: snapshot,
        })
    }

    /// Arm a session: serializes the arm, assigns a new generation, and defers `listen.armed`
    /// until the DSP confirms the exact tuple.
    pub fn arm(&mut self, session: SessionEpoch, request: ListenArm) -> Result<(), ControlError> {
        let current_audio = self.audio_epoch;

        if request.context.session_epoch != session {
            return Err(ControlError::Unauthorized);
        }
        if request.context.audio_epoch != current_audio {
            return Err(ControlError::StaleEpoch);
        }

        // Validate against the current state, then release the borrow before calling the DSP.
        let (new_generation, context) = {
            let state = self.state_mut(session);
            if request.context.safety_generation != state.generation {
                return Err(ControlError::StaleEpoch);
            }
            if state.used_nonces.contains(&request.arm_nonce) {
                return Err(ControlError::StaleEpoch);
            }
            let new_generation = SafetyGeneration(state.generation.0 + 1);
            let context = SessionContext {
                session_epoch: session,
                audio_epoch: current_audio,
                safety_generation: new_generation,
            };
            (new_generation, context)
        };

        let arm_for_dsp = ListenArm {
            context,
            applied_revision: request.applied_revision,
            arm_nonce: request.arm_nonce,
        };
        self.dsp.request_arm(arm_for_dsp)?;

        let state = self.state_mut(session);
        state.cancel_pending_arm();
        state.generation = new_generation;
        state.pending_arm = Some((request.arm_nonce, context));
        Ok(())
    }

    /// Disarm a session. Priority: closes the shared gate before returning.
    ///
    /// A stale-generation disarm for the authenticated current session is applied, not rejected;
    /// the generation is still advanced so a stale worker cannot later arm.
    pub fn disarm(
        &mut self,
        session: SessionEpoch,
        request: ListenDisarm,
    ) -> Result<(), ControlError> {
        if request.session_epoch != session {
            return Err(ControlError::Unauthorized);
        }
        let state = self.state_mut(session);
        state.cancel_pending_arm();
        let new_generation = SafetyGeneration(state.generation.0 + 1);
        state.generation = new_generation;
        self.dsp.disarm(session, new_generation);
        Ok(())
    }

    /// Validate a typed audio event and, for a genuinely installed revision, emit `mix.applied`.
    ///
    /// `mix.applied` is emitted only for the revision the actor last installed; skipped or
    /// coalesced revisions and stale generations are dropped.
    pub fn on_audio_event(&mut self, ev: AudioEvent) -> Option<ServerMessage> {
        match ev {
            AudioEvent::MixApplied {
                applied_revision,
                context,
                start_sample,
                snapshot,
            } => {
                let state = self.sessions.get_mut(&context.session_epoch)?;
                if context.audio_epoch != self.audio_epoch {
                    return None;
                }
                if context.safety_generation != state.generation {
                    return None;
                }
                if applied_revision != state.installed_revision {
                    return None;
                }
                state.applied_snapshot = Some(snapshot);
                Some(ServerMessage::MixApplied(MixApplied {
                    applied_revision,
                    context,
                    start_sample,
                    snapshot,
                }))
            }
            AudioEvent::ArmApplied { context, arm_nonce } => {
                let state = self.sessions.get_mut(&context.session_epoch)?;
                let (pending_nonce, pending_context) = state.pending_arm?;
                if pending_nonce != arm_nonce || pending_context != context {
                    return None;
                }
                if state.cancelled_nonces.contains(&arm_nonce)
                    || state.used_nonces.contains(&arm_nonce)
                {
                    return None;
                }
                state.pending_arm = None;
                state.armed = true;
                state.remember_nonce(arm_nonce);
                Some(ServerMessage::ListenArmed(ListenArmed {
                    session_epoch: context.session_epoch,
                    safety_generation: context.safety_generation,
                    audio_epoch: context.audio_epoch,
                    arm_nonce,
                }))
            }
            AudioEvent::Interrupted { session_epoch, .. } => {
                if let Some(state) = self.sessions.get_mut(&session_epoch) {
                    state.armed = false;
                    state.cancel_pending_arm();
                }
                None
            }
            AudioEvent::Fault { .. } => None,
        }
    }

    /// Retrieve a previously accepted patch acknowledgement (boundary idempotency).
    pub fn cached_ack(&mut self, session: SessionEpoch, request_id: &str) -> Option<MixAck> {
        let state = self.sessions.get_mut(&session)?;
        state
            .cache
            .iter()
            .find(|(id, _)| id == request_id)
            .map(|(_, ack)| *ack)
    }

    /// Record an accepted patch acknowledgement for idempotent retries.
    pub fn remember_ack(&mut self, session: SessionEpoch, request_id: &str, ack: MixAck) {
        let state = self.state_mut(session);
        if state.cache.iter().any(|(id, _)| id == request_id) {
            return;
        }
        state.cache.push_back((request_id.to_string(), ack));
        while state.cache.len() > REQUEST_CACHE_LIMIT {
            state.cache.pop_front();
        }
    }
}
