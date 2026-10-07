/**
 * Browser receiver ports composed from real signaling, WebRTC media, gesture activation, a
 * monotonic clock, and WebRTC statistics.
 *
 * This module is the LAN bundle's composition root: it is the only place the musician page wires
 * the frozen {@link ReceiverPorts} to browser APIs. It intentionally imports nothing from
 * `@tauri-apps/api` and nothing from the admin/preview bundles.
 */

import type { ActivationPort, ReceiverPorts } from '../receiver-ports';
import { createBrowserMediaPort } from '../transport/media';
import { createMusicianSignaling, type SignalingDeps } from '../transport/signaling';
import { createStatsPort } from '../transport/stats';

/** Injectable seams; production passes nothing and uses the browser globals. */
export interface BrowserPortsOptions extends SignalingDeps {
  /** Document used for gesture listeners and the playback element. */
  document?: Document;
  /** Monotonic clock in milliseconds (defaults to `performance.now`). */
  clock?: () => number;
  /** WebRTC readiness timeout in milliseconds. */
  connectionTimeoutMs?: number;
  /** `RTCPeerConnection` constructor; injectable for tests. */
  rtcPeerConnection?: typeof RTCPeerConnection;
}

/**
 * A trusted-gesture activation source.
 *
 * A gesture is trusted only while the tab is visible: `isActive()` is true after a genuine
 * `pointerdown`/`keydown`/`touchstart`/`click`, and false once the document is hidden so a
 * backgrounded tab cannot silently reopen the local gate.
 */
export function createActivationPort(doc: Document): ActivationPort {
  const handlers = new Set<() => void>();
  let active = false;

  const mark = (): void => {
    active = true;
    for (const handler of handlers) handler();
  };

  const isVisible = (): boolean =>
    typeof doc.visibilityState === 'string' ? doc.visibilityState !== 'hidden' : true;

  const onVisibility = (): void => {
    if (!isVisible()) active = false;
  };

  if (typeof doc.addEventListener === 'function') {
    doc.addEventListener('pointerdown', mark);
    doc.addEventListener('keydown', mark);
    doc.addEventListener('touchstart', mark);
    doc.addEventListener('click', mark);
    doc.addEventListener('visibilitychange', onVisibility);
  }

  return {
    onUserGesture(handler) {
      handlers.add(handler);
      return () => {
        handlers.delete(handler);
      };
    },
    isActive() {
      return active && isVisible();
    },
  };
}

/** A monotonic millisecond clock, falling back to `Date.now()` when `performance` is absent. */
function defaultClock(): () => number {
  const perf = globalThis.performance;
  if (perf && typeof perf.now === 'function') {
    return () => Math.floor(perf.now());
  }
  return () => Date.now();
}

/**
 * Build the full receiver port set for the phone browser.
 *
 * Throws when a required browser primitive is missing (no `RTCPeerConnection`, no `WebSocket`, no
 * `document`) so the caller can render the honest unavailable state instead of a fake controller.
 */
export function createBrowserReceiverPorts(options: BrowserPortsOptions = {}): ReceiverPorts {
  const doc = options.document ?? globalThis.document;
  if (!doc) throw new Error('A document is required for the browser receiver');
  if (!options.webSocket && !globalThis.WebSocket) {
    throw new Error('WebSocket is not available in this browser');
  }
  if (!options.rtcPeerConnection && !globalThis.RTCPeerConnection) {
    throw new Error('WebRTC is not available in this browser');
  }

  const clock = options.clock ?? defaultClock();
  const signaling = createMusicianSignaling(options);
  const media = createBrowserMediaPort(signaling, {
    rtcPeerConnection: options.rtcPeerConnection,
    document: doc,
    connectionTimeoutMs: options.connectionTimeoutMs,
  });
  const stats = createStatsPort({
    peerConnection: () => media.peerConnection(),
    sessionEpoch: () => signaling.currentSessionEpoch(),
    audioEpoch: () => signaling.currentAudioEpoch(),
    clock,
  });
  const activation = createActivationPort(doc);

  return { signaling, media, activation, clock, stats };
}
