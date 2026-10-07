/**
 * Mobile latency diagnostics (ARCHITECTURE §17.1).
 *
 * These are pure functions over normalized browser samples. They must never:
 * - halve RTT to claim one-way latency,
 * - add RTT and buffer delay into a single "total",
 * - fabricate `0` for missing data.
 *
 * A first sample (or a changed session/stats id/stream/epoch) is `'Waiting for sample'`; missing,
 * zero-emitted, stale, or unsupported data is `'Unavailable'`. End-to-end latency is always
 * `'Not measured'` unless a physical analog measurement exists — which no browser statistic is.
 */

import type { DiagnosticsSnapshot, InboundStats } from '../receiver-ports';

/** The documented non-numeric diagnostics states. */
export type Unavailable = 'Unavailable';
export type WaitingForSample = 'Waiting for sample';

/**
 * Selected ICE-candidate-pair round-trip time in milliseconds.
 *
 * `currentRoundTripTime` is in seconds per the WebRTC stats spec. This is the **latest reported
 * STUN round trip**; a display refresh does not imply a fresh measurement.
 */
export function computeNetworkRttMs(
  pair: { currentRoundTripTime: number } | undefined,
): number | Unavailable {
  if (!pair) return 'Unavailable';
  const seconds = pair.currentRoundTripTime;
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) {
    return 'Unavailable';
  }
  return seconds * 1000;
}

/** Whether a sample can be compared against a previous one. */
function sameInterval(prev: InboundStats, next: InboundStats): boolean {
  return (
    prev.sessionEpoch === next.sessionEpoch &&
    prev.statsId === next.statsId &&
    prev.stream === next.stream &&
    prev.epoch === next.epoch
  );
}

/**
 * Latest-interval average jitter-buffer hold, in milliseconds.
 *
 * `1000 * Δ(jitterBufferDelay) / Δ(jitterBufferEmittedCount)`, computed only when the samples
 * belong to the same session/stats id/stream/epoch and the deltas are finite, non-negative, with
 * a positive emitted count. Anything else is `'Unavailable'`; a first or reset interval is
 * `'Waiting for sample'`.
 */
export function computeBufferDelayMs(
  previous: InboundStats | null,
  current: InboundStats | undefined,
): number | Unavailable | WaitingForSample {
  if (!current) return 'Unavailable';
  if (!previous) return 'Waiting for sample';
  if (!sameInterval(previous, current)) return 'Waiting for sample';

  const emittedDelta = current.emitted - previous.emitted;
  const delayDelta = current.delay - previous.delay;

  if (!Number.isFinite(emittedDelta) || !Number.isFinite(delayDelta)) return 'Unavailable';
  // A counter reset or a stalled stream is not a measurement.
  if (emittedDelta <= 0 || delayDelta < 0) return 'Unavailable';

  const ms = (1000 * delayDelta) / emittedDelta;
  if (!Number.isFinite(ms) || ms < 0) return 'Unavailable';
  return ms;
}

/** Assemble the snapshot. End-to-end is never derived from network statistics. */
export function buildDiagnostics(
  networkRttMs: number | Unavailable,
  bufferDelayMs: number | Unavailable | WaitingForSample,
  lastUpdated: number | null = null,
): DiagnosticsSnapshot {
  return {
    network_rtt_ms: networkRttMs,
    buffer_delay_ms: bufferDelayMs,
    end_to_end: 'Not measured',
    last_updated: lastUpdated,
  };
}

/** The diagnostics before any sample has been observed. */
export const EMPTY_DIAGNOSTICS: DiagnosticsSnapshot = buildDiagnostics('Unavailable', 'Unavailable', null);
