/**
 * WebRTC media statistics over `RTCPeerConnection.getStats()`.
 *
 * Every read is served from the cache populated by {@link StatsPort.refresh}; `refresh()` never
 * overlaps itself even when the diagnostics interval fires faster than a sample completes. Only
 * genuinely present statistics are surfaced: a missing candidate pair or inbound report yields
 * `undefined`, never a fabricated zero. `jitterBufferDelay` is reported by the browser in seconds,
 * matching {@link InboundStats.delay}.
 */

import type { InboundStats, StatsPort } from '../receiver-ports';

/** Minimal view of the peer connection the port reads; keeps this module framework-free. */
interface StatsSource {
  getStats(): Promise<RTCStatsReport>;
}

export interface StatsDeps {
  /** Returns the live peer connection, or `null` when none is open. */
  peerConnection(): StatsSource | null;
  /** The current authenticated session epoch, for sample normalization. */
  sessionEpoch(): string | null;
  /** The current audio epoch, for sample normalization. */
  audioEpoch(): string | null;
  /** Monotonic clock in milliseconds. */
  clock(): number;
}

/** A candidate-pair report with a usable RTT. */
interface CandidatePairStats {
  currentRoundTripTime: number;
}

interface InboundRtpStats {
  id?: string;
  kind?: string;
  jitterBufferDelay?: number;
  jitterBufferEmittedCount?: number;
  trackIdentifier?: string;
  mediaType?: string;
}

/** A `getStats()` report with a `forEach`-friendly `Map`-like shape. */
function reportEntries(report: RTCStatsReport): [string, unknown][] {
  const entries: [string, unknown][] = [];
  report.forEach((value, key) => {
    entries.push([String(key), value]);
  });
  return entries;
}

/** A finite, non-negative number, or `undefined`. */
function finiteOrUndefined(value: unknown): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return undefined;
  return value;
}

/** The selected, nominated ICE candidate pair, when the browser reports one. */
function selectCandidatePair(report: RTCStatsReport): CandidatePairStats | undefined {
  const entries = reportEntries(report);
  for (const [, value] of entries) {
    const stats = value as Record<string, unknown>;
    if (stats.type !== 'candidate-pair') continue;
    const nominated = stats.nominated === true;
    const state = stats.state;
    const rtt = finiteOrUndefined(stats.currentRoundTripTime);
    if (rtt === undefined) continue;
    // Prefer a succeeded+nominated pair; fall back to any succeeded pair with an RTT.
    if (state === 'succeeded' && nominated) return { currentRoundTripTime: rtt };
  }
  for (const [, value] of entries) {
    const stats = value as Record<string, unknown>;
    if (stats.type !== 'candidate-pair' || stats.state !== 'succeeded') continue;
    const rtt = finiteOrUndefined(stats.currentRoundTripTime);
    if (rtt !== undefined) return { currentRoundTripTime: rtt };
  }
  return undefined;
}

/** The active audio inbound-rtp report, when present. */
function selectInbound(report: RTCStatsReport): InboundRtpStats | undefined {
  for (const [, value] of reportEntries(report)) {
    const stats = value as Record<string, unknown>;
    if (stats.type !== 'inbound-rtp') continue;
    const kind = stats.kind ?? stats.mediaType;
    if (kind !== 'audio') continue;
    return stats as InboundRtpStats;
  }
  return undefined;
}

export function createStatsPort(deps: StatsDeps): StatsPort {
  let pair: { currentRoundTripTime: number } | undefined;
  let inboundSample: InboundStats | undefined;
  let refreshing: Promise<void> | null = null;

  /** Normalize one audio inbound report; missing/zero counters return `undefined`. */
  function normalize(stats: InboundRtpStats): InboundStats | undefined {
    const emitted = finiteOrUndefined(stats.jitterBufferEmittedCount);
    const delay = finiteOrUndefined(stats.jitterBufferDelay);
    const statsId = typeof stats.id === 'string' ? stats.id : undefined;
    // Without the identities diagnostics compares on, a sample cannot be used honestly.
    if (statsId === undefined) return undefined;
    if (emitted === undefined || delay === undefined) return undefined;

    const stream = typeof stats.trackIdentifier === 'string' ? stats.trackIdentifier : '';
    return {
      sessionEpoch: deps.sessionEpoch() ?? '',
      statsId,
      stream,
      epoch: deps.audioEpoch() ?? '',
      emitted,
      delay,
      sampledAtMs: deps.clock(),
    };
  }

  return {
    selectedCandidateRtt(): { currentRoundTripTime: number } | undefined {
      return pair;
    },

    inbound(): InboundStats | undefined {
      return inboundSample;
    },

    refresh(): Promise<void> {
      if (refreshing) return refreshing;
      refreshing = (async () => {
        try {
          const connection = deps.peerConnection();
          if (!connection) {
            pair = undefined;
            inboundSample = undefined;
            return;
          }
          const report = await connection.getStats();
          const nextPair = selectCandidatePair(report);
          const nextInbound = selectInbound(report);
          pair = nextPair;
          inboundSample = nextInbound ? normalize(nextInbound) : undefined;
        } catch {
          // A failed read leaves the previous cache intact rather than inventing values.
        } finally {
          refreshing = null;
        }
      })();
      return refreshing;
    },
  };
}
