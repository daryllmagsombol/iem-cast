/**
 * Browser receive-only audio over WebRTC.
 *
 * Exactly one `recvonly` audio transceiver is created: no microphone (`getUserMedia` is never
 * called), no video, and no data channel. The offer is sent through the authenticated WSS control
 * channel (`rtc.offer`); the host's `rtc.answer` is applied and ICE candidates are trickled in both
 * directions. Readiness resolves from the real connection/ICE state, never from a timer alone.
 *
 * Playback uses a hidden, looped `<audio>` element. Audio only starts inside a user gesture
 * (`play()` is invoked from the gesture and the element begins muted); `mute()` toggles the local
 * element without touching the peer connection.
 */

import type { RtcAnswer, RtcCandidate } from '../protocol';
import type {
  AudioMediaPort,
  JitterBufferPreference,
  JitterBufferPreferenceStatus,
  JitterBufferTargetProperty,
} from '../receiver-ports';
import { buildJitterBufferPreference } from './diagnostics';
import type { BrowserMusicianSignaling } from './signaling';

/** How long to wait for a live ICE/DTLS connection before failing honestly. */
const CONNECTION_TIMEOUT_MS = 30_000;

/**
 * A receiver that may expose either buffer-target preference: `jitterBufferTarget` (milliseconds)
 * or the seconds-valued `playoutDelayHint` fallback.
 */
type BufferTargetReceiver = RTCRtpReceiver & {
  jitterBufferTarget?: number;
  playoutDelayHint?: number;
};

/** Injectable environment seams; production uses the browser globals. */
export interface MediaDeps {
  /** `RTCPeerConnection` constructor; injectable for tests. */
  rtcPeerConnection?: typeof RTCPeerConnection;
  /** Document used to host the playback element; injectable for tests. */
  document?: Document;
  /** Connection readiness timeout in milliseconds. */
  connectionTimeoutMs?: number;
}

export interface BrowserAudioMediaPort extends AudioMediaPort {
  /** The playback element, for diagnostics/tests; `null` before creation. */
  audioElement(): HTMLAudioElement | null;
  /** The live peer connection, or `null`; the stats port reads `getStats()` from it. */
  peerConnection(): RTCPeerConnection | null;
  /** Raw jitter-buffer preference feature-detect/assignment outcome, or `undefined` when unset. */
  jitterBufferStatus(): JitterBufferPreference;
}

/** Tracks whether the local element is currently silenced. */
function isConnectedState(state: RTCPeerConnectionState): boolean {
  return state === 'connected';
}

function isFailedState(state: RTCPeerConnectionState): boolean {
  return state === 'failed' || state === 'closed';
}

export function createBrowserMediaPort(
  signaling: BrowserMusicianSignaling,
  deps: MediaDeps = {},
): BrowserAudioMediaPort {
  const PeerConnection = deps.rtcPeerConnection ?? globalThis.RTCPeerConnection;
  const doc = deps.document ?? globalThis.document;
  const connectionTimeoutMs = deps.connectionTimeoutMs ?? CONNECTION_TIMEOUT_MS;

  let pc: RTCPeerConnection | null = null;
  let transceiver: RTCRtpTransceiver | null = null;
  let audio: HTMLAudioElement | null = null;
  let readiness: Promise<void> | null = null;
  let cancelReadiness: (() => void) | null = null;
  let muted = true;
  // Whether the audio sink has actually started; the buffer target only applies once it is playing.
  let playing = false;
  let unsubscribe: (() => void) | null = null;
  // Raw feature-detect/assignment outcome for the current receiver; reset with the connection.
  let jitterStatus: JitterBufferPreferenceStatus | undefined;

  function ensureAudioElement(): HTMLAudioElement {
    if (audio) return audio;
    const element = doc.createElement('audio');
    element.loop = true;
    element.autoplay = false;
    element.muted = muted;
    element.setAttribute('playsinline', '');
    element.style.display = 'none';
    doc.body?.appendChild(element);
    audio = element;
    return element;
  }

  function closePeerConnection(): void {
    if (cancelReadiness) {
      cancelReadiness();
      cancelReadiness = null;
    }
    if (unsubscribe) {
      unsubscribe();
      unsubscribe = null;
    }
    if (transceiver) {
      try {
        transceiver.stop();
      } catch {
        // A stopped/closed transceiver is already inert.
      }
      transceiver = null;
    }
    if (pc) {
      try {
        pc.close();
      } catch {
        // Closing an already-closed connection is not an error.
      }
      pc = null;
    }
    if (audio) {
      audio.pause();
      audio.srcObject = null;
    }
    // Playback stopped, so the buffer target can no longer take effect until play() succeeds again.
    playing = false;
    // A new connection gets a new receiver; the old preference outcome no longer applies.
    jitterStatus = undefined;
    readiness = null;
  }

  function applyAnswer(answer: RtcAnswer): void {
    if (!pc) return;
    void pc
      .setRemoteDescription({ type: 'answer', sdp: answer.sdp })
      .catch(() => undefined);
  }

  function applyCandidate(candidate: RtcCandidate): void {
    if (!pc) return;
    const init: RTCIceCandidateInit =
      candidate.candidate === null
        ? { candidate: '' }
        : {
            candidate: candidate.candidate,
            sdpMid: candidate.sdpMid ?? undefined,
            sdpMLineIndex: candidate.sdpMLineIndex ?? undefined,
          };
    void pc.addIceCandidate(init).catch(() => undefined);
  }

  /**
   * Whether `property` is present on `receiver`, or `undefined` when the feature-detect itself
   * threw. `in` checks presence without invoking a getter; only a `Proxy` `has` trap can throw.
   */
  function detectProperty(receiver: object, property: string): boolean | undefined {
    try {
      return property in receiver;
    } catch {
      return undefined;
    }
  }

  /**
   * Assign one buffer-target property and record exactly what was observable. `value` is in the
   * property's own unit (`jitterBufferTarget` ms; `playoutDelayHint` seconds). Assignment not
   * throwing means only that the browser accepted the preference, never that the actual buffer is
   * the requested value or that latency dropped.
   */
  function assignBufferTarget(
    receiver: BufferTargetReceiver,
    property: JitterBufferTargetProperty,
    requestedMs: number,
    value: number,
  ): boolean {
    try {
      if (property === 'jitterBufferTarget') receiver.jitterBufferTarget = value;
      else receiver.playoutDelayHint = value;
      jitterStatus = { supported: true, requestedMs, accepted: true, property };
      return true;
    } catch {
      jitterStatus = { supported: true, requestedMs, accepted: false, property };
      return false;
    }
  }

  /**
   * Apply the requested target to the observed receiver. `jitterBufferTarget` (ms) is preferred;
   * the seconds-valued `playoutDelayHint` is the fallback for browsers that lack it. Never invents
   * a capability claim: no receiver, or a throwing `has` trap, records `undefined` (surfacing as
   * `'Unavailable'`), while a receiver that genuinely lacks both properties records `'Unsupported'`.
   */
  function applyBufferTarget(ms: number): boolean {
    const receiver = transceiver?.receiver as BufferTargetReceiver | undefined;
    if (!receiver) {
      // Nothing was observed: make no capability claim (`jitterStatus` stays undefined).
      jitterStatus = undefined;
      return false;
    }
    const hasJitterTarget = detectProperty(receiver, 'jitterBufferTarget');
    if (hasJitterTarget === undefined) {
      // A throwing `has` trap is an unknown capability, not an observed absence.
      jitterStatus = undefined;
      return false;
    }
    if (hasJitterTarget) {
      return assignBufferTarget(receiver, 'jitterBufferTarget', ms, ms);
    }
    const hasPlayoutHint = detectProperty(receiver, 'playoutDelayHint');
    if (hasPlayoutHint === undefined) {
      jitterStatus = undefined;
      return false;
    }
    if (!hasPlayoutHint) {
      // The receiver exists and genuinely lacks both properties: that IS an observed absence.
      jitterStatus = { supported: false, requestedMs: ms, accepted: false, property: null };
      return false;
    }
    // `playoutDelayHint` is seconds, so the millisecond request is converted.
    return assignBufferTarget(receiver, 'playoutDelayHint', ms, ms / 1000);
  }

  /**
   * Re-assert the last requested target once the sink is playing. Bounded and idempotent: a no-op
   * unless a target was requested, a supported property is present, and playback has started. Safe
   * to call at most once per diagnostics refresh — there is no internal retry loop.
   */
  function reassertBufferTarget(): boolean {
    if (!playing || jitterStatus?.supported !== true || jitterStatus.requestedMs === null) {
      return false;
    }
    return applyBufferTarget(jitterStatus.requestedMs);
  }

  return {
    async createRecvOnlyAudio(signal?: AbortSignal): Promise<void> {
      signal?.throwIfAborted();
      if (pc && readiness) return readiness;
      if (!PeerConnection) {
        throw new Error('WebRTC is not available in this browser');
      }

      const connection = new PeerConnection();
      pc = connection;
      const isCurrent = () => pc === connection && !signal?.aborted;

      // One receive-only audio section; no microphone, no video, no data channel.
      const audioTransceiver = connection.addTransceiver('audio', { direction: 'recvonly' });
      transceiver = audioTransceiver;

      const element = ensureAudioElement();

      connection.ontrack = event => {
        if (!isCurrent()) return;
        const [stream] = event.streams;
        if (stream) element.srcObject = stream;
      };

      connection.onicecandidate = event => {
        if (!isCurrent()) return;
        signaling.sendRtcCandidate(event.candidate);
      };

      // Subscribe to host signaling before offering so the answer/candidates cannot race the offer.
      unsubscribe = signaling.onEvent(event => {
        if (!isCurrent()) return;
        if (event.type === 'rtc.answer') {
          applyAnswer(event.payload as RtcAnswer);
        } else if (event.type === 'rtc.candidate') {
          applyCandidate(event.payload as RtcCandidate);
        }
      });

      readiness = new Promise<void>((resolve, reject) => {
        let settled = false;
        const finish = (error?: Error): void => {
          if (settled) return;
          settled = true;
          cancelReadiness = null;
          clearTimeout(timer);
          if (error) reject(error);
          else resolve();
        };
        // A `stop()` while connecting cancels silently (resolves) so no rejection leaks.
        cancelReadiness = () => finish();
        const timer = setTimeout(
          () => finish(new Error('WebRTC connection timed out')),
          connectionTimeoutMs,
        );

        const evaluate = (): void => {
          const state = connection.connectionState;
          const ice = connection.iceConnectionState;
          if (isConnectedState(state) || ice === 'connected' || ice === 'completed') {
            finish();
          } else if (isFailedState(state) || ice === 'failed') {
            finish(new Error('WebRTC connection failed'));
          }
        };
        connection.onconnectionstatechange = evaluate;
        connection.oniceconnectionstatechange = evaluate;
      });

      // Attach the readiness rejection handler immediately so it is never an unhandled rejection
      // while the offer/local-description work is still in flight.
      readiness.catch(() => undefined);

      const offer = await connection.createOffer();
      if (!isCurrent()) return;
      await connection.setLocalDescription(offer);
      if (!isCurrent()) return;
      await signaling.sendRtcOffer(connection.localDescription?.sdp ?? offer.sdp ?? '', signal);
      if (!isCurrent()) return;

      return readiness;
    },

    async setJitterBufferTargetMs(ms: number): Promise<boolean> {
      return applyBufferTarget(ms);
    },

    reassertJitterBufferTarget(): boolean {
      return reassertBufferTarget();
    },

    async play(): Promise<void> {
      const element = ensureAudioElement();
      element.muted = muted;
      await element.play();
      // The receiver-buffer target only takes effect while the sink is playing; re-assert now.
      playing = true;
      reassertBufferTarget();
    },

    stop(): void {
      closePeerConnection();
    },

    mute(next: boolean): void {
      muted = next;
      if (audio) audio.muted = next;
    },

    audioElement(): HTMLAudioElement | null {
      return audio;
    },

    peerConnection(): RTCPeerConnection | null {
      return pc;
    },

    jitterBufferStatus(): JitterBufferPreference {
      // No receiver or no request yet: `buildJitterBufferPreference(undefined)` yields the honest
      // `'Unavailable'`, not an invented `'Unsupported'` or a fabricated `'Accepted'`.
      return buildJitterBufferPreference(jitterStatus);
    },
  };
}
