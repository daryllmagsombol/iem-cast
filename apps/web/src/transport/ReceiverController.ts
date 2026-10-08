/**
 * Browser receiver controller (Task 6).
 *
 * Owns the phone's listening lifecycle over injected ports so the LAN bundle never imports
 * `@tauri-apps/api`. The core invariant is the **sticky local safety gate**:
 *
 * - personal master mute is local and immediate, always;
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

  // The nonce of the current arm attempt; cleared on stop/disarm so late replies are ignored.
  let currentArmNonce: string | null = null;
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
    snapshot = { ...snapshot, error: message };
    emit();
  }

  /** Close the local output AND the logical gate. Always safe, always immediate. */
  function closeLocalGate(): void {
    ports.media.mute(true);
    snapshot = { ...snapshot, master_local_muted: true };
  }

  function handleEvent(event: ServerEvent): void {
    switch (eventType(event)) {
      case 'catalog.snapshot': {
        const payload = event.payload as ReceiverSnapshot['catalog'];
        snapshot = { ...snapshot, catalog: payload };
        emit();
        break;
      }
      case 'session.snapshot': {
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
        snapshot = {
          ...snapshot,
          phase: payload.phase,
          requested_mix: payload.requestedMix ?? snapshot.requested_mix,
          accepted_mix: payload.acceptedMix ?? snapshot.accepted_mix,
          applied_mix: payload.appliedMix ?? snapshot.applied_mix,
        };
        emit();
        break;
      }
      case 'mix.ack': {
        // Accepted only. The gate is untouched.
        const payload = event.payload as MixAck;
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
        // Ignore replies for a cancelled attempt or a superseded generation.
        if (currentArmNonce === null || payload.armNonce !== currentArmNonce) break;
        currentGeneration = payload.safetyGeneration;
        if (typeof payload.audioEpoch === 'string') hostAudioEpoch = payload.audioEpoch;
        phase('armed');
        // The gate stays closed until playback readiness is confirmed below.
        break;
      }
      case 'error': {
        const payload = event.payload as { message?: string };
        setError(payload.message ?? 'Host error');
        break;
      }
      default:
        break;
    }
  }

  function refreshDiagnostics(): void {
    void ports.stats.refresh().then(() => {
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
    currentArmNonce = null;
    currentGeneration = null;
  }

  async function ensureConnected(): Promise<void> {
    if (snapshot.phase === 'unpaired') {
      phase('negotiating');
      // Subscribe BEFORE connecting: the host pushes its initial `session.snapshot` and
      // `catalog.snapshot` the moment the socket opens, so subscribing afterwards would drop them
      // and leave the phone with no catalog ("No sources available") forever.
      if (!unsubscribeSignaling) unsubscribeSignaling = ports.signaling.onEvent(handleEvent);
      try {
        await ports.signaling.connect();
        await ports.media.createRecvOnlyAudio();
        await ports.media.setJitterBufferTargetMs(0);
        startDiagnostics();
        phase('ready-muted');
      } catch (error) {
        // Do not leave a subscription attached to a connection that never succeeded.
        if (unsubscribeSignaling) {
          unsubscribeSignaling();
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
      try {
        await ensureConnected();
      } catch (error) {
        setError(error instanceof Error ? error.message : 'Connection failed');
        phase('interrupted');
        throw error;
      }
    },

    async requestMix(patch: MixPatch) {
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
      emit();
      // Resolves on ACCEPTED, never on DSP-applied. The ack itself is the acceptance, so record
      // it here as well as in the pushed `mix.ack` event path.
      const ack = await ports.signaling.requestMix(patch);
      snapshot = { ...snapshot, accepted_mix: ack.canonicalSettings };
      emit();
      return ack;
    },

    async arm() {
      // The gate stays closed until the whole sequence below validates.
      closeLocalGate();
      setError(null);

      if (!ports.activation.isActive()) {
        throw new Error('An explicit user gesture is required to start listening');
      }

      // Playback must begin inside the gesture's activation window, so start it BEFORE any await
      // (including connecting). The promise is awaited once the rest of the setup completes.
      const playPromise = ports.media.play();

      await ensureConnected();
      await playPromise;

      const accepted = snapshot.accepted_mix ?? snapshot.requested_mix;
      if (hostAudioEpoch === null) {
        // Without the host's real audio epoch the arm would be rejected as STALE_EPOCH. Refuse
        // locally rather than sending a fabricated value that can never succeed.
        throw new Error('The host has not reported its audio epoch yet; reconnect and try again');
      }

      const nonce = crypto.randomUUID();
      currentArmNonce = nonce;
      const arm: ListenArm = {
        audioEpoch: hostAudioEpoch as ListenArm['audioEpoch'],
        safetyGeneration: (currentGeneration ?? '0') as CounterString,
        // A freshly paired listener has no mix yet, so there is no accepted revision to reference.
        // The host's initial revision is "0"; sending it is correct and lets the listener start
        // listening before touching any fader.
        appliedRevision: (accepted?.mixRevision ?? '0') as CounterString,
        armNonce: nonce as unknown as ListenArm['armNonce'],
      };

      await ports.signaling.arm(arm);
      // `listen.armed` arrives asynchronously; only its matching reply opens the gate.
    },

    personalMasterMute(muted: boolean) {
      // Muting is ALWAYS immediate and local, regardless of arm state.
      if (muted) {
        closeLocalGate();
        return;
      }
      // Unmuting requires a gesture, an armed session, and readiness. Otherwise stay closed.
      if (!ports.activation.isActive() || snapshot.phase !== 'armed') {
        closeLocalGate();
        return;
      }
      ports.media.mute(false);
      snapshot = { ...snapshot, master_local_muted: false };
      emit();
    },

    stop() {
      // Immediate local silence; no ack wait.
      cancelArmAttempt();
      closeLocalGate();
      ports.media.stop();
      phase('ready-muted');
    },

    disconnect() {
      cancelArmAttempt();
      closeLocalGate();
      ports.media.stop();
      stopDiagnostics();
      if (unsubscribeSignaling) {
        unsubscribeSignaling();
        unsubscribeSignaling = null;
      }
      phase('revoked');
    },
  };

  return controller;
}

export type { AudioMediaPort };
