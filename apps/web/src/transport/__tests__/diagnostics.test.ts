import { describe, expect, test } from 'vitest';

import {
  buildDiagnostics,
  computeBufferDelayMs,
  computeNetworkRttMs,
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
});
