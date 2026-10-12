import { describe, expect, test } from 'vitest';

import {
  buildDiagnostics,
  buildJitterBufferPreference,
  computeBufferDelayMs,
  computeConcealmentPercent,
  computeMinimumDelayMs,
  computeNetworkRttMs,
  computePacketsLostDelta,
  computeTargetDelayMs,
} from '../diagnostics';
import type { InboundStats } from '../../receiver-ports';

function inbound(overrides: Partial<InboundStats> = {}): InboundStats {
  return {
    sessionEpoch: 's',
    statsId: 'a',
    stream: 'audio',
    epoch: 'e',
    emitted: 100,
    delay: 1.0,
    sampledAtMs: 1000,
    ...overrides,
  };
}

describe('computeNetworkRttMs', () => {
  test('reads the selected pair as currentRoundTripTime * 1000', () => {
    expect(computeNetworkRttMs({ currentRoundTripTime: 0.042 })).toBeCloseTo(42, 6);
  });

  test('missing or invalid data is Unavailable, never zero', () => {
    expect(computeNetworkRttMs(undefined)).toBe('Unavailable');
    expect(computeNetworkRttMs(undefined)).not.toBe(0);
    expect(computeNetworkRttMs({ currentRoundTripTime: Number.NaN })).toBe('Unavailable');
    expect(computeNetworkRttMs({ currentRoundTripTime: -1 })).toBe('Unavailable');
  });
});

describe('computeBufferDelayMs', () => {
  test('first sample is Waiting for sample', () => {
    expect(computeBufferDelayMs(null, inbound())).toBe('Waiting for sample');
  });

  test('a valid interval computes 1000 * dDelay / dEmitted', () => {
    const prev = inbound({ emitted: 100, delay: 1.0 });
    const next = inbound({ emitted: 200, delay: 1.2, sampledAtMs: 2000 });
    expect(computeBufferDelayMs(prev, next)).toBeCloseTo(2.0, 6);
  });

  test('zero emitted delta is Unavailable, not Waiting and not zero', () => {
    const prev = inbound({ emitted: 100, delay: 1.0 });
    const same = inbound({ emitted: 100, delay: 1.0, sampledAtMs: 2000 });
    expect(computeBufferDelayMs(prev, same)).toBe('Unavailable');
  });

  test('a changed stats id resets to Waiting for sample', () => {
    const prev = inbound({ statsId: 'a' });
    const moved = inbound({ statsId: 'b', emitted: 200, delay: 1.2 });
    expect(computeBufferDelayMs(prev, moved)).toBe('Waiting for sample');
  });

  test('a changed session or epoch resets to Waiting for sample', () => {
    expect(computeBufferDelayMs(inbound({ sessionEpoch: 's' }), inbound({ sessionEpoch: 't' }))).toBe(
      'Waiting for sample',
    );
    expect(computeBufferDelayMs(inbound({ epoch: 'e' }), inbound({ epoch: 'f' }))).toBe(
      'Waiting for sample',
    );
  });

  test('a counter reset (emitted went backwards) is Unavailable', () => {
    const prev = inbound({ emitted: 200, delay: 1.2 });
    const reset = inbound({ emitted: 3, delay: 0.1, sampledAtMs: 2000 });
    expect(computeBufferDelayMs(prev, reset)).toBe('Unavailable');
  });

  test('missing current is Unavailable', () => {
    expect(computeBufferDelayMs(inbound(), undefined)).toBe('Unavailable');
  });
});

describe('computeTargetDelayMs and computeMinimumDelayMs', () => {
  test('latest-interval target/minimum delay in ms use the same ratio guards', () => {
    const prev = inbound({ emitted: 100, targetDelay: 0.5, minimumDelay: 0.1 });
    const next = inbound({ emitted: 200, targetDelay: 0.7, minimumDelay: 0.3, sampledAtMs: 2000 });
    expect(computeTargetDelayMs(prev, next)).toBeCloseTo(2.0, 6);
    expect(computeMinimumDelayMs(prev, next)).toBeCloseTo(2.0, 6);
  });

  test('absent target/minimum fields are Unavailable, never 0', () => {
    const prev = inbound({ emitted: 100 });
    const next = inbound({ emitted: 200, sampledAtMs: 2000 });
    expect(computeTargetDelayMs(prev, next)).toBe('Unavailable');
    expect(computeMinimumDelayMs(prev, next)).toBe('Unavailable');
    expect(computeTargetDelayMs(prev, next)).not.toBe(0);
  });

  test('first sample is Waiting for sample and a changed id resets', () => {
    expect(computeTargetDelayMs(null, inbound({ targetDelay: 0.5 }))).toBe('Waiting for sample');
    expect(
      computeTargetDelayMs(inbound({ statsId: 'a', targetDelay: 0.5 }), inbound({ statsId: 'b', emitted: 200, targetDelay: 0.7 })),
    ).toBe('Waiting for sample');
  });

  test('zero emitted delta is Unavailable', () => {
    const prev = inbound({ emitted: 100, targetDelay: 0.5 });
    const same = inbound({ emitted: 100, targetDelay: 0.5, sampledAtMs: 2000 });
    expect(computeTargetDelayMs(prev, same)).toBe('Unavailable');
  });

  test('a valid target reading survives a missing minimum (availability is independent)', () => {
    const prev = inbound({ emitted: 100, targetDelay: 0.5 });
    const next = inbound({ emitted: 200, targetDelay: 0.7, sampledAtMs: 2000 });
    expect(computeTargetDelayMs(prev, next)).toBeCloseTo(2.0, 6);
    expect(computeMinimumDelayMs(prev, next)).toBe('Unavailable');
  });
});

describe('computeConcealmentPercent', () => {
  test('is 100 * dConcealedSamples / dTotalSamplesReceived', () => {
    const prev = inbound({ totalSamplesReceived: 1000, concealedSamples: 10 });
    const next = inbound({ totalSamplesReceived: 2000, concealedSamples: 60, sampledAtMs: 2000 });
    expect(computeConcealmentPercent(prev, next)).toBeCloseTo(5, 6);
  });

  test('first sample is Waiting for sample', () => {
    expect(computeConcealmentPercent(null, inbound({ totalSamplesReceived: 1000, concealedSamples: 1 }))).toBe(
      'Waiting for sample',
    );
  });

  test('zero total-samples delta is Unavailable, never 0 or 100', () => {
    const prev = inbound({ totalSamplesReceived: 1000, concealedSamples: 10 });
    const same = inbound({ totalSamplesReceived: 1000, concealedSamples: 10, sampledAtMs: 2000 });
    expect(computeConcealmentPercent(prev, same)).toBe('Unavailable');
  });

  test('a negative concealed delta is Unavailable', () => {
    const prev = inbound({ totalSamplesReceived: 1000, concealedSamples: 60 });
    const next = inbound({ totalSamplesReceived: 2000, concealedSamples: 10, sampledAtMs: 2000 });
    expect(computeConcealmentPercent(prev, next)).toBe('Unavailable');
  });

  test('concealed samples exceeding total samples is an impossible counter, Unavailable (not >100, not clamped)', () => {
    const prev = inbound({ totalSamplesReceived: 1000, concealedSamples: 10 });
    // An asymmetric counter reset: concealed grows by 5000 while total grows by only 100.
    const next = inbound({ totalSamplesReceived: 1100, concealedSamples: 5010, sampledAtMs: 2000 });
    expect(computeConcealmentPercent(prev, next)).toBe('Unavailable');
    expect(computeConcealmentPercent(prev, next)).not.toBe(100);
  });

  test('missing concealment counters are Unavailable', () => {
    const prev = inbound({ totalSamplesReceived: 1000 });
    const next = inbound({ totalSamplesReceived: 2000, sampledAtMs: 2000 });
    expect(computeConcealmentPercent(prev, next)).toBe('Unavailable');
  });
});

describe('computePacketsLostDelta', () => {
  test('a non-negative per-interval delta is reported, including a genuine 0', () => {
    const prev = inbound({ packetsReceived: 100, packetsLost: 2 });
    const next = inbound({ packetsReceived: 200, packetsLost: 5, sampledAtMs: 2000 });
    expect(computePacketsLostDelta(prev, next)).toBe(3);
    expect(
      computePacketsLostDelta(inbound({ packetsLost: 2 }), inbound({ packetsLost: 2 })),
    ).toBe(0);
  });

  test('a same-interval counter decrease is Unavailable, never a fabricated 0', () => {
    const prev = inbound({ packetsLost: 5 });
    const decreased = inbound({ packetsLost: 2, sampledAtMs: 2000 });
    expect(computePacketsLostDelta(prev, decreased)).toBe('Unavailable');
    expect(computePacketsLostDelta(prev, decreased)).not.toBe(0);
  });

  test('first sample is Waiting for sample and a changed identity resets', () => {
    expect(computePacketsLostDelta(null, inbound({ packetsLost: 2 }))).toBe('Waiting for sample');
    expect(
      computePacketsLostDelta(inbound({ stream: 'a', packetsLost: 1 }), inbound({ stream: 'b', packetsLost: 3 })),
    ).toBe('Waiting for sample');
  });

  test('missing packetsLost on either side is Unavailable', () => {
    expect(computePacketsLostDelta(inbound({}), inbound({ packetsLost: 2 }))).toBe('Unavailable');
    expect(computePacketsLostDelta(inbound({ packetsLost: 2 }), inbound({}))).toBe('Unavailable');
  });
});

describe('buildJitterBufferPreference', () => {
  test('an absent status is Unavailable, never an invented unsupported claim', () => {
    expect(buildJitterBufferPreference(undefined)).toEqual({ state: 'Unavailable', requested_ms: null });
  });

  test('a feature-detect failure is Unsupported, not Rejected', () => {
    expect(buildJitterBufferPreference({ supported: false, requestedMs: 0, accepted: false })).toEqual({
      state: 'Unsupported',
      requested_ms: 0,
    });
  });

  test('an accepted assignment is Accepted (a preference, never a forced buffer)', () => {
    expect(buildJitterBufferPreference({ supported: true, requestedMs: 0, accepted: true })).toEqual({
      state: 'Accepted',
      requested_ms: 0,
    });
  });

  test('a throwing assignment on a supported receiver is Rejected', () => {
    expect(buildJitterBufferPreference({ supported: true, requestedMs: 0, accepted: false })).toEqual({
      state: 'Rejected',
      requested_ms: 0,
    });
  });

  test('a supported receiver that was never asked is Not requested', () => {
    expect(buildJitterBufferPreference({ supported: true, requestedMs: null, accepted: false })).toEqual({
      state: 'Not requested',
      requested_ms: null,
    });
  });
});

describe('buildDiagnostics', () => {
  test('never halves RTT and never sums RTT plus buffer', () => {
    const snap = buildDiagnostics(42, 2, 1234);
    expect(snap.end_to_end).toBe('Not measured');
    expect(Object.prototype.hasOwnProperty.call(snap, 'oneWay')).toBe(false);
    expect(Object.prototype.hasOwnProperty.call(snap, 'total')).toBe(false);
    expect(snap.network_rtt_ms).toBe(42);
    expect(snap.buffer_delay_ms).toBe(2);
  });

  test('end-to-end stays Not measured even when both statistics exist', () => {
    expect(buildDiagnostics(10, 1).end_to_end).toBe('Not measured');
  });

  test('optional metrics default to Unavailable and the preference is Not requested', () => {
    const snap = buildDiagnostics(42, 2);
    expect(snap.buffer_target_delay_ms).toBe('Unavailable');
    expect(snap.buffer_minimum_delay_ms).toBe('Unavailable');
    expect(snap.packets_lost_delta).toBe('Unavailable');
    expect(snap.concealment_percent).toBe('Unavailable');
    expect(snap.jitter_buffer_preference).toEqual({ state: 'Not requested', requested_ms: null });
  });

  test('a valid 152 ms target delay is reported independently and never as end-to-end', () => {
    const snap = buildDiagnostics(42, 2, 1234, {
      bufferTargetDelayMs: 152,
      jitterBufferPreference: { state: 'Accepted', requested_ms: 0 },
    });
    expect(snap.buffer_target_delay_ms).toBe(152);
    // RTT is reported separately and is not mixed into the buffer target.
    expect(snap.network_rtt_ms).toBe(42);
    expect(snap.end_to_end).toBe('Not measured');
  });
});
