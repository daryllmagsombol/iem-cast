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

import type {
  DiagnosticsSnapshot,
  InboundStats,
  JitterBufferPreference,
} from '../receiver-ports';

/** The documented non-numeric diagnostics states. */
export type Unavailable = 'Unavailable';
export type WaitingForSample = 'Waiting for sample';

/** The value shape of an optional latest-interval metric. */
export type IntervalMetric = number | Unavailable | WaitingForSample;

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
  return computeIntervalAverage(previous, current, sample => sample.delay, sample => sample.emitted);
}

/**
 * Shared guard for latest-interval averages: `1000 * Δ(numerator) / Δ(denominator)`.
 *
 * The identity guard (`sessionEpoch`/`statsId`/`stream`/`epoch`) and the interval rules are the
 * same for every optional metric, so availability is independent per metric: a missing numerator
 * or denominator yields `'Unavailable'` without erasing a valid neighbour.
 */
function computeIntervalAverage(
  previous: InboundStats | null,
  current: InboundStats | undefined,
  numeratorOf: (sample: InboundStats) => number | undefined,
  denominatorOf: (sample: InboundStats) => number | undefined,
): IntervalMetric {
  if (!current) return 'Unavailable';
  if (!previous) return 'Waiting for sample';
  if (!sameInterval(previous, current)) return 'Waiting for sample';

  const numerator = numeratorOf(current);
  const previousNumerator = numeratorOf(previous);
  const denominator = denominatorOf(current);
  const previousDenominator = denominatorOf(previous);
  if (
    numerator === undefined ||
    previousNumerator === undefined ||
    denominator === undefined ||
    previousDenominator === undefined
  ) {
    return 'Unavailable';
  }

  const denominatorDelta = denominator - previousDenominator;
  const numeratorDelta = numerator - previousNumerator;
  if (!Number.isFinite(denominatorDelta) || !Number.isFinite(numeratorDelta)) return 'Unavailable';
  // A counter reset or a stalled stream is not a measurement.
  if (denominatorDelta <= 0 || numeratorDelta < 0) return 'Unavailable';

  const ms = (1000 * numeratorDelta) / denominatorDelta;
  if (!Number.isFinite(ms) || ms < 0) return 'Unavailable';
  return ms;
}

/**
 * Latest-interval average {@link RTCInboundRtpStreamStats.jitterBufferTargetDelay} in ms.
 *
 * Uses the same identity/interval guards as {@link computeBufferDelayMs}. This is the browser's
 * accumulated target hold; it is **not** the actual current buffer and is not a latency guarantee.
 */
export function computeTargetDelayMs(
  previous: InboundStats | null,
  current: InboundStats | undefined,
): IntervalMetric {
  return computeIntervalAverage(previous, current, sample => sample.targetDelay, sample => sample.emitted);
}

/**
 * Latest-interval average `jitterBufferMinimumDelay` in ms: the minimum delay achievable from
 * network characteristics alone. Same guards as {@link computeBufferDelayMs}.
 */
export function computeMinimumDelayMs(
  previous: InboundStats | null,
  current: InboundStats | undefined,
): IntervalMetric {
  return computeIntervalAverage(previous, current, sample => sample.minimumDelay, sample => sample.emitted);
}

/**
 * Latest-interval concealment percentage: `100 * Δ(concealedSamples) / Δ(totalSamplesReceived)`.
 *
 * Requires the same identity and a strictly positive denominator delta with a finite, non-negative
 * concealed delta. A genuine `0%` is a real measurement; missing counters, a zero denominator
 * delta, a counter decrease, or a reset are `'Unavailable'`/`'Waiting for sample'`.
 */
export function computeConcealmentPercent(
  previous: InboundStats | null,
  current: InboundStats | undefined,
): IntervalMetric {
  if (!current) return 'Unavailable';
  if (!previous) return 'Waiting for sample';
  if (!sameInterval(previous, current)) return 'Waiting for sample';

  const { concealedSamples, totalSamplesReceived } = current;
  const previousConcealed = previous.concealedSamples;
  const previousTotal = previous.totalSamplesReceived;
  if (
    concealedSamples === undefined ||
    totalSamplesReceived === undefined ||
    previousConcealed === undefined ||
    previousTotal === undefined
  ) {
    return 'Unavailable';
  }

  const totalDelta = totalSamplesReceived - previousTotal;
  const concealedDelta = concealedSamples - previousConcealed;
  if (!Number.isFinite(totalDelta) || !Number.isFinite(concealedDelta)) return 'Unavailable';
  if (totalDelta <= 0 || concealedDelta < 0) return 'Unavailable';

  const percent = (100 * concealedDelta) / totalDelta;
  if (!Number.isFinite(percent) || percent < 0) return 'Unavailable';
  return percent;
}

/**
 * Latest-interval lost-packet delta for a live interval.
 *
 * `packetsLost` is **signed** in the spec (it can go negative), so a same-interval decrease is a
 * counter reset, not a measurement: it is `'Unavailable'`, never a fabricated `0`. A genuine
 * unchanged count is a real `0`. Requires the same identity and both counters present and finite.
 */
export function computePacketsLostDelta(
  previous: InboundStats | null,
  current: InboundStats | undefined,
): number | Unavailable | WaitingForSample {
  if (!current) return 'Unavailable';
  if (!previous) return 'Waiting for sample';
  if (!sameInterval(previous, current)) return 'Waiting for sample';

  const { packetsLost } = current;
  const previousLost = previous.packetsLost;
  if (packetsLost === undefined || previousLost === undefined) return 'Unavailable';
  if (!Number.isFinite(packetsLost) || !Number.isFinite(previousLost)) return 'Unavailable';

  const delta = packetsLost - previousLost;
  if (!Number.isFinite(delta) || delta < 0) return 'Unavailable';
  return delta;
}

/** The preference reported when a port does not expose any status: honestly `'Unavailable'`. */
export const UNKNOWN_JITTER_BUFFER_PREFERENCE: JitterBufferPreference = {
  state: 'Unavailable',
  requested_ms: null,
};

/**
 * Turn the media port's raw jitter-buffer status into the snapshot's preference record.
 *
 * An absent status means the port did not expose the capability (an older/fake port), so the
 * honest label is `'Unavailable'` — never an invented `'Unsupported'`. `'Accepted'` means only
 * that the assignment did not throw; the browser may still choose any actual target.
 */
export function buildJitterBufferPreference(
  status: { supported: boolean; requestedMs: number | null; accepted: boolean } | undefined,
): JitterBufferPreference {
  if (!status) return { ...UNKNOWN_JITTER_BUFFER_PREFERENCE };
  if (!status.supported) return { state: 'Unsupported', requested_ms: status.requestedMs };
  if (status.requestedMs === null) return { state: 'Not requested', requested_ms: null };
  if (status.accepted) return { state: 'Accepted', requested_ms: status.requestedMs };
  return { state: 'Rejected', requested_ms: status.requestedMs };
}

/** The optional latest-interval metrics and preference assembled alongside RTT/buffer. */
export interface OptionalDiagnostics {
  bufferTargetDelayMs?: IntervalMetric;
  bufferMinimumDelayMs?: IntervalMetric;
  packetsLostDelta?: IntervalMetric;
  concealmentPercent?: IntervalMetric;
  jitterBufferPreference?: JitterBufferPreference;
}

/** Assemble the snapshot. End-to-end is never derived from network statistics. */
export function buildDiagnostics(
  networkRttMs: number | Unavailable,
  bufferDelayMs: IntervalMetric,
  lastUpdated: number | null = null,
  optional: OptionalDiagnostics = {},
): DiagnosticsSnapshot {
  return {
    network_rtt_ms: networkRttMs,
    buffer_delay_ms: bufferDelayMs,
    buffer_target_delay_ms: optional.bufferTargetDelayMs ?? 'Unavailable',
    buffer_minimum_delay_ms: optional.bufferMinimumDelayMs ?? 'Unavailable',
    packets_lost_delta: optional.packetsLostDelta ?? 'Unavailable',
    concealment_percent: optional.concealmentPercent ?? 'Unavailable',
    jitter_buffer_preference: optional.jitterBufferPreference ?? { state: 'Not requested', requested_ms: null },
    end_to_end: 'Not measured',
    last_updated: lastUpdated,
  };
}

/** The diagnostics before any sample has been observed. */
export const EMPTY_DIAGNOSTICS: DiagnosticsSnapshot = buildDiagnostics('Unavailable', 'Unavailable', null);
