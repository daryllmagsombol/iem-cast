/**
 * Browser receiver controller (Task 6).
 *
 * Owns the phone's listening lifecycle over injected ports so the LAN bundle never imports
 * `@tauri-apps/api`. The core invariant is the **sticky local safety gate**:
 *
 * - personal master mute closes local output immediately; explicit unmute also releases host mute;
 * - the gate only opens on an explicit, current user gesture PLUS an armed session with the
 *   current safety generation PLUS playback readiness;
 * - acknowledgements, reconnects, preset recovery, and transport recovery never reopen it;
 * - a cancelled arm nonce is permanently invalid for that attempt.
 *
 * Accepted and DSP-applied are distinct: `requestMix` resolves on the accepted ack only.
 */

import type {
  CounterString,
  ListenArm,
  ListenerPhase,
  MixAck,
  MixPatch,
  MixSnapshot,
  ServerEvent,
} from '../protocol';
import type {
  AudioMediaPort,
  DiagnosticsSnapshot,
  ReceiverController,
  ReceiverPorts,
  ReceiverSnapshot,
} from '../receiver-ports';
import {
  buildDiagnostics,
  computeBufferDelayMs,
  computeNetworkRttMs,
  EMPTY_DIAGNOSTICS,
} from './diagnostics';

/** Freshness window for diagnostics: stale readings become `Unavailable`. */
const DIAGNOSTICS_STALE_MS = 3000;
const ARM_TIMEOUT_MS = 10000;

/** The empty, disconnected snapshot used before anything connects. */
function emptySnapshot(): ReceiverSnapshot {
  return {
    phase: 'unpaired',
    catalog: { catalogRevision: '0' as CounterString, sources: [] },
    requested_mix: null,
    accepted_mix: null,
    applied_mix: null,
    master_local_muted: true,
    diagnostics: EMPTY_DIAGNOSTICS,
    error: null,
  };
}

/** Narrow a server event to its payload without re-validating (the transport already parsed it). */
function eventType(event: ServerEvent): string {
  return event.type;
}

export function createReceiverController(ports: ReceiverPorts): ReceiverController {
  const listeners = new Set<(snapshot: ReceiverSnapshot) => void>();
  let snapshot = emptySnapshot();
  let lifecycle = new AbortController();
  let sessionKey: string | null = null;
  let lastSentArm: { nonce: string; signal: AbortSignal; session: string | null } | null = null;

  // The nonce of the current arm attempt; cleared on stop/disarm so late replies are ignored.
  let currentArmNonce: string | null = null;
  type ArmAttempt = {
    nonce: string;
    sent: boolean;
    resolve: () => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
  };
  let pendingArm: ArmAttempt | null = null;
  // The generation the host last confirmed; replies must match it to arm.
  let currentGeneration: string | null = null;
  // The host's real audio epoch, learned from `session.snapshot`/`listen.armed`. Arming must send
  // this exact value; the host rejects a mismatch with STALE_EPOCH.
  let hostAudioEpoch: string | null = null;
  // The most recent applied mix, and the accepted one, kept distinct from the request.
  let previousInbound: ReturnType<ReceiverPorts['stats']['inbound']> | null = null;
  let diagnostics: DiagnosticsSnapshot = EMPTY_DIAGNOSTICS;
  let diagnosticsTimer: ReturnType<typeof setInterval> | null = null;
  let unsubscribeSignaling: (() => void) | null = null;
  type MixBatch = {
    patch: MixPatch;
    signal: AbortSignal;
    requestId: string;
    canceled?: boolean;
    waiters: { resolve: (ack: MixAck) => void; reject: (error: unknown) => void }[];
  };
  let queuedMix: MixBatch | null = null;
  let mixFlight: Promise<void> | null = null;
  let activeMix: MixBatch | null = null;
  let mixError: string | null = null;
  let outputGesture = 0;

  function emit(): void {
    const copy = snapshot;
    for (const listener of listeners) listener(copy);
  }

  function phase(next: ListenerPhase): void {
    if (snapshot.phase === next) return;
    snapshot = { ...snapshot, phase: next };
    emit();
  }

  function setError(message: string | null): void {
    mixError = null;
    snapshot = { ...snapshot, error: message };
    emit();
  }

  /** Close the local output AND the logical gate. Always safe, always immediate. */
  function closeLocalGate(): void {
    outputGesture++;
    ports.media.mute(true);
    snapshot = { ...snapshot, master_local_muted: true, master_unmute_pending: false };
  }

  // One wire request at a time. Queued patches are absolute desired settings, so the latest
  // replaces older queued settings, but every caller waits for that batch's acceptance.
  function sendQueuedMix(): void {
    if (mixFlight || !queuedMix) return;
    const batch = queuedMix;
    activeMix = batch;
    queuedMix = null;
    const patch = {
      ...batch.patch,
      baseRevision: snapshot.accepted_mix?.mixRevision ?? batch.patch.baseRevision,
    };
    mixFlight = (async () => {
      try {
        const ack = await ports.signaling.requestMix(patch, batch.signal, batch.requestId);
        if (batch.canceled || batch.signal.aborted) return;
        // A newer authoritative session snapshot wins over a delayed request result.
        if (snapshot.accepted_mix && BigInt(ack.acceptedRevision) < BigInt(snapshot.accepted_mix.mixRevision)) {
          for (const waiter of batch.waiters) waiter.reject(new Error('Mix request superseded by host settings'));
          return;
        }
        snapshot = {
          ...snapshot,
          accepted_mix: ack.canonicalSettings,
          requested_mix: queuedMix ? snapshot.requested_mix : ack.canonicalSettings,
          error: snapshot.error === mixError ? null : snapshot.error,
        };
        mixError = null;
        emit();
        for (const waiter of batch.waiters) waiter.resolve(ack);
      } catch (error) {
        if (batch.canceled || batch.signal.aborted) return;
        mixError = error instanceof Error ? error.message : 'The host could not accept this mix.';
        snapshot = {
          ...snapshot,
          requested_mix: queuedMix ? snapshot.requested_mix : snapshot.accepted_mix,
          error: mixError,
        };
        emit();
        for (const waiter of batch.waiters) waiter.reject(error);
      } finally {
        mixFlight = null;
        activeMix = null;
        sendQueuedMix();
      }
    })();
  }

  function cancelMixRequests(): void {
    const error = new Error('Mix request canceled');
    for (const batch of [activeMix, queuedMix]) {
      if (!batch) continue;
      batch.canceled = true;
      for (const waiter of batch.waiters) waiter.reject(error);
      batch.waiters = [];
    }
    queuedMix = null;
    snapshot = { ...snapshot, requested_mix: snapshot.accepted_mix };
  }

  async function waitForMix(): Promise<void> {
    while (mixFlight) await mixFlight;
  }

  function handleEvent(event: ServerEvent): void {
    const key = typeof event.hostEpoch === 'string' && typeof event.sessionEpoch === 'string'
      ? `${event.hostEpoch}:${event.sessionEpoch}` : null;
    if (event.type !== 'session.snapshot' && key && sessionKey && key !== sessionKey) return;
    switch (eventType(event)) {
      case 'catalog.snapshot': {
        const payload = event.payload as ReceiverSnapshot['catalog'];
        snapshot = { ...snapshot, catalog: payload };
        emit();
        break;
      }
      case 'session.snapshot': {
        if (key && sessionKey && key !== sessionKey) {
          invalidateLifecycle();
          currentGeneration = null;
          hostAudioEpoch = null;
        }
        if (key) sessionKey = key;
        const payload = event.payload as {
          phase: ListenerPhase;
          context?: { audioEpoch?: string; safetyGeneration?: string };
          requestedMix: MixSnapshot | null;
          acceptedMix: MixSnapshot | null;
          appliedMix: MixSnapshot | null;
        };
        // The host's real audio epoch arrives here; arming must send exactly this value.
        if (typeof payload.context?.audioEpoch === 'string') {
          hostAudioEpoch = payload.context.audioEpoch;
        }
        if (typeof payload.context?.safetyGeneration === 'string') {
          currentGeneration = payload.context.safetyGeneration;
        }
        snapshot = {
          ...snapshot,
          // Host authority is not local playback permission. Only a live confirmed attempt can
          // put this lifecycle into armed, including when a snapshot arrives after a timeout.
          phase: payload.phase === 'armed' && (currentArmNonce === null || pendingArm !== null)
            ? snapshot.phase : payload.phase,
          requested_mix: payload.requestedMix === undefined ? snapshot.requested_mix : payload.requestedMix,
          accepted_mix: payload.acceptedMix === undefined ? snapshot.accepted_mix : payload.acceptedMix,
          applied_mix: payload.appliedMix === undefined ? snapshot.applied_mix : payload.appliedMix,
        };
        emit();
        break;
      }
      case 'mix.ack': {
        // Accepted only. The gate is untouched.
        if (!activeMix || activeMix.canceled || activeMix.signal.aborted ||
          event.requestId !== activeMix.requestId) break;
        const payload = event.payload as MixAck;
        if (snapshot.accepted_mix && BigInt(payload.acceptedRevision) < BigInt(snapshot.accepted_mix.mixRevision)) break;
        snapshot = {
          ...snapshot,
          accepted_mix: payload.canonicalSettings,
        };
        emit();
        break;
      }
      case 'mix.applied': {
        // DSP-confirmed. Recorded for display; still never opens the gate.
        const payload = event.payload as { settings: MixSnapshot };
        snapshot = { ...snapshot, applied_mix: payload.settings };
        emit();
        break;
      }
      case 'listen.armed': {
        const payload = event.payload as {
          armNonce: string;
          safetyGeneration: CounterString;
          audioEpoch?: string;
        };
        // A scoped late confirmation may update host authority, but never grants permission.
        if (lastSentArm && !lastSentArm.signal.aborted && lastSentArm.session === sessionKey &&
          payload.armNonce === lastSentArm.nonce) {
          if (currentGeneration === null || BigInt(payload.safetyGeneration) >= BigInt(currentGeneration)) {
            currentGeneration = payload.safetyGeneration;
            if (typeof payload.audioEpoch === 'string') hostAudioEpoch = payload.audioEpoch;
          }
        }
        if (!pendingArm?.sent || payload.armNonce !== pendingArm.nonce ||
          payload.armNonce !== currentArmNonce) break;
        const attempt = pendingArm;
        phase('armed');
        finishArm(attempt);
        break;
      }
      case 'error': {
        const payload = event.payload as { message?: string };
        setError(payload.message ?? 'Host error');
        if (pendingArm?.sent && event.requestId === pendingArm.nonce) {
          finishArm(pendingArm, new Error(payload.message ?? 'The host could not arm this session.'));
        }
        break;
      }
      default:
        break;
    }
  }

  function refreshDiagnostics(): void {
    const signal = lifecycle.signal;
    void ports.stats.refresh().then(() => {
      if (signal.aborted) return;
      const pair = ports.stats.selectedCandidateRtt();
      const inbound = ports.stats.inbound();
      const rtt = computeNetworkRttMs(pair);
      const buffer = computeBufferDelayMs(previousInbound ?? null, inbound);
      if (inbound) previousInbound = inbound;

      const now = ports.clock();
      const stale =
        diagnostics.last_updated !== null && now - diagnostics.last_updated > DIAGNOSTICS_STALE_MS;
      const next = buildDiagnostics(
        rtt,
        stale && buffer !== 'Waiting for sample' ? 'Unavailable' : buffer,
        now,
      );
      diagnostics = next;
      snapshot = { ...snapshot, diagnostics: next };
      emit();
    });
  }

  function startDiagnostics(): void {
    if (diagnosticsTimer !== null) return;
    diagnosticsTimer = setInterval(refreshDiagnostics, 1000);
  }

  function stopDiagnostics(): void {
    if (diagnosticsTimer !== null) {
      clearInterval(diagnosticsTimer);
      diagnosticsTimer = null;
    }
  }

  /** Drop a pending arm attempt so its late reply can never arm the session. */
  function cancelArmAttempt(): void {
    if (pendingArm) finishArm(pendingArm, new Error('Listening attempt canceled'));
    currentArmNonce = null;
  }

  function invalidateLifecycle(): void {
    lifecycle.abort();
    lifecycle = new AbortController();
    lastSentArm = null;
    cancelArmAttempt();
    cancelMixRequests();
    closeLocalGate();
  }

  function finishArm(attempt: ArmAttempt, error?: Error): void {
    if (pendingArm !== attempt) return;
    clearTimeout(attempt.timer);
    pendingArm = null;
    if (error) {
      currentArmNonce = null;
      closeLocalGate();
      attempt.reject(error);
    } else attempt.resolve();
  }

  async function ensureConnected(signal: AbortSignal): Promise<void> {
    signal.throwIfAborted();
    if (snapshot.phase === 'unpaired') {
      phase('negotiating');
      // Subscribe BEFORE connecting: the host pushes its initial `session.snapshot` and
      // `catalog.snapshot` the moment the socket opens, so subscribing afterwards would drop them
      // and leave the phone with no catalog ("No sources available") forever.
      let setupComplete = false;
      if (!unsubscribeSignaling) unsubscribeSignaling = ports.signaling.onEvent(event => {
        if (!setupComplete && signal.aborted) return;
        handleEvent(event);
      });
      const subscription = unsubscribeSignaling;
      try {
        await ports.signaling.connect(signal);
        signal.throwIfAborted();
        await ports.media.createRecvOnlyAudio(signal);
        signal.throwIfAborted();
        await ports.media.setJitterBufferTargetMs(0);
        signal.throwIfAborted();
        setupComplete = true;
        startDiagnostics();
        phase('ready-muted');
      } catch (error) {
        // Do not leave a subscription attached to a connection that never succeeded.
        if (unsubscribeSignaling === subscription) {
          subscription?.();
          unsubscribeSignaling = null;
        }
        throw error;
      }
    }
  }

  const controller: ReceiverController = {
    subscribe(handler) {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },

    getSnapshot() {
      return snapshot;
    },

    async connect() {
      const signal = lifecycle.signal;
      try {
        await ensureConnected(signal);
      } catch (error) {
        if (signal.aborted) throw error;
        setError(error instanceof Error ? error.message : 'Connection failed');
        phase('interrupted');
        throw error;
      }
    },

    requestMix(patch: MixPatch) {
      // Optimistic local echo of intent; kept distinct from accepted and applied.
      snapshot = {
        ...snapshot,
        requested_mix: {
          catalogRevision: patch.catalogRevision,
          mixRevision: patch.baseRevision,
          sources: patch.sources,
          masterDb: patch.masterDb,
          masterMuted: patch.masterMuted,
        },
      };
      snapshot = { ...snapshot, error: snapshot.error === mixError ? null : snapshot.error };
      mixError = null;
      const result = new Promise<MixAck>((resolve, reject) => {
        if (queuedMix) {
          queuedMix.patch = patch;
          queuedMix.waiters.push({ resolve, reject });
        } else {
          queuedMix = { patch, signal: lifecycle.signal, requestId: crypto.randomUUID(), waiters: [{ resolve, reject }] };
        }
      });
      emit();
      sendQueuedMix();
      return result;
    },

    arm() {
      if (pendingArm) return Promise.reject(new Error('A listening attempt is already in progress'));
      if (snapshot.phase === 'armed') return Promise.reject(new Error('This session is already armed'));
      // The gate stays closed until the whole sequence below validates.
      closeLocalGate();
      setError(null);

      if (!ports.activation.isActive()) {
        return Promise.reject(new Error('An explicit user gesture is required to start listening'));
      }

      const nonce = crypto.randomUUID();
      const signal = lifecycle.signal;
      currentArmNonce = nonce;
      let attempt!: ArmAttempt;
      const result = new Promise<void>((resolve, reject) => {
        attempt = { nonce, sent: false, resolve, reject, timer: setTimeout(() => {
          if (pendingArm !== attempt) return;
          const error = new Error('Listening confirmation timed out; try starting again.');
          setError(error.message);
          finishArm(attempt, error);
        }, ARM_TIMEOUT_MS) };
      });
      pendingArm = attempt;
      const checkAttempt = () => {
        signal.throwIfAborted();
        if (currentArmNonce !== nonce) throw new Error('Listening attempt canceled');
      };

      // Playback must begin inside the gesture's activation window, so start it BEFORE any await
      // (including connecting). The promise is awaited once the rest of the setup completes.
      let playPromise: Promise<void>;
      try {
        playPromise = ports.media.play();
        // Attach immediately so playback rejection during host setup is not unhandled.
        void playPromise.catch(() => {});
      } catch (error) {
        finishArm(attempt, error instanceof Error ? error : new Error('Playback failed'));
        return result;
      }

      void (async () => {
        try {
          await ensureConnected(signal);
          checkAttempt();
          await playPromise;
          checkAttempt();

          if (hostAudioEpoch === null) {
            throw new Error('The host has not reported its audio epoch yet; reconnect and try again');
          }

          // A requested echo is not acceptance. Register a safe, muted mix if none is accepted.
          await waitForMix();
          checkAttempt();
          let accepted = snapshot.accepted_mix;
          if (accepted === null) {
            const neutral: MixPatch = {
              baseRevision: '0' as CounterString,
              catalogRevision: snapshot.catalog.catalogRevision,
              sources: snapshot.catalog.sources
                .filter(source => source.available && source.authorized)
                .map(source => ({ sourceId: source.sourceId, gainDb: -60, muted: true })),
              masterDb: -60,
              masterMuted: true,
            };
            const ack = await controller.requestMix(neutral);
            checkAttempt();
            accepted = ack.canonicalSettings;
          }

          const arm: ListenArm = {
            audioEpoch: hostAudioEpoch as ListenArm['audioEpoch'],
            safetyGeneration: (currentGeneration ?? '0') as CounterString,
            appliedRevision: accepted.mixRevision,
            armNonce: nonce as unknown as ListenArm['armNonce'],
          };

          attempt.sent = true;
          lastSentArm = { nonce, signal, session: sessionKey };
          await ports.signaling.arm(arm, nonce, signal);
          checkAttempt();
          // Sending is not confirmation. Only the matching event completes this attempt.
        } catch (error) {
          finishArm(attempt, error instanceof Error ? error : new Error('Could not start listening'));
        }
      })();
      return result;
    },

    personalMasterMute(muted: boolean) {
      // Muting is ALWAYS immediate and local, regardless of arm state.
      if (muted) {
        closeLocalGate();
        emit();
        return;
      }
      // Unmuting requires a gesture, an armed session, and readiness. Otherwise stay closed.
      if (!ports.activation.isActive() || snapshot.phase !== 'armed' ||
        currentArmNonce === null || pendingArm !== null) {
        closeLocalGate();
        return;
      }
      const desired = snapshot.requested_mix ?? snapshot.accepted_mix;
      if (!desired) {
        closeLocalGate();
        return;
      }
      closeLocalGate();
      snapshot = { ...snapshot, master_unmute_pending: true };
      const gesture = outputGesture;
      const nonce = currentArmNonce;
      const generation = currentGeneration;
      // Capture the trusted gesture now; an unrelated ack can never open the gate. A later mute,
      // stop, or arm invalidates this permission while host acceptance is pending.
      void controller.requestMix({
        baseRevision: snapshot.accepted_mix?.mixRevision ?? desired.mixRevision,
        catalogRevision: snapshot.catalog.catalogRevision,
        sources: desired.sources,
        masterDb: desired.masterDb,
        masterMuted: false,
      }).then(ack => {
        if (gesture !== outputGesture) return;
        snapshot = { ...snapshot, master_unmute_pending: false };
        if (snapshot.phase !== 'armed' ||
          nonce !== currentArmNonce || generation !== currentGeneration ||
          ack.canonicalSettings.masterMuted) {
          emit();
          return;
        }
        ports.media.mute(false);
        snapshot = { ...snapshot, master_local_muted: false, master_unmute_pending: false };
        emit();
      }).catch(() => {
        // requestMix records the rejection; the local gate remains closed.
        if (gesture === outputGesture) {
          snapshot = { ...snapshot, master_unmute_pending: false };
          emit();
        }
      });
    },

    stop() {
      // Immediate local silence; no ack wait.
      invalidateLifecycle();
      ports.media.stop();
      snapshot = { ...snapshot, phase: 'ready-muted' };
      emit();
    },

    disconnect() {
      invalidateLifecycle();
      ports.media.stop();
      stopDiagnostics();
      if (unsubscribeSignaling) {
        unsubscribeSignaling();
        unsubscribeSignaling = null;
      }
      snapshot = { ...snapshot, phase: 'revoked' };
      emit();
    },
  };

  return controller;
}

export type { AudioMediaPort };
