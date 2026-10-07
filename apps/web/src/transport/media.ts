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
import type { AudioMediaPort } from '../receiver-ports';
import type { BrowserMusicianSignaling } from './signaling';

/** How long to wait for a live ICE/DTLS connection before failing honestly. */
const CONNECTION_TIMEOUT_MS = 30_000;

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
  let unsubscribe: (() => void) | null = null;

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

  return {
    async createRecvOnlyAudio(): Promise<void> {
      if (pc && readiness) return readiness;
      if (!PeerConnection) {
        throw new Error('WebRTC is not available in this browser');
      }

      const connection = new PeerConnection();
      pc = connection;

      // One receive-only audio section; no microphone, no video, no data channel.
      const audioTransceiver = connection.addTransceiver('audio', { direction: 'recvonly' });
      transceiver = audioTransceiver;

      const element = ensureAudioElement();

      connection.ontrack = event => {
        const [stream] = event.streams;
        if (stream) element.srcObject = stream;
      };

      connection.onicecandidate = event => {
        signaling.sendRtcCandidate(event.candidate);
      };

      // Subscribe to host signaling before offering so the answer/candidates cannot race the offer.
      unsubscribe = signaling.onEvent(event => {
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
      await connection.setLocalDescription(offer);
      await signaling.sendRtcOffer(connection.localDescription?.sdp ?? offer.sdp ?? '');

      return readiness;
    },

    async setJitterBufferTargetMs(ms: number): Promise<boolean> {
      const receiver = transceiver?.receiver as
        | (RTCRtpReceiver & { jitterBufferTarget?: number })
        | undefined;
      if (!receiver || !('jitterBufferTarget' in receiver)) return false;
      try {
        receiver.jitterBufferTarget = ms;
        return true;
      } catch {
        return false;
      }
    },

    async play(): Promise<void> {
      const element = ensureAudioElement();
      element.muted = muted;
      await element.play();
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
  };
}
