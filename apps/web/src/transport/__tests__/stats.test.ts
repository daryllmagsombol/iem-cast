import { describe, expect, test } from 'vitest';

import { createStatsPort } from '../stats';

/** Build a `getStats()`-shaped report from raw records. */
function report(records: [string, Record<string, unknown>][]): RTCStatsReport {
  const map = new Map<string, unknown>(records);
  return map as unknown as RTCStatsReport;
}

function makePort(reportValue: RTCStatsReport, overrides: Partial<{ pc: boolean }> = {}) {
  let calls = 0;
  const port = createStatsPort({
    peerConnection: () =>
      overrides.pc === false
        ? null
        : {
            getStats: async () => {
              calls += 1;
              return reportValue;
            },
          },
    sessionEpoch: () => 'session-1',
    audioEpoch: () => 'audio-1',
    clock: () => 1234,
  });
  return { port, calls: () => calls };
}

const inboundRecord: Record<string, unknown> = {
  type: 'inbound-rtp',
  kind: 'audio',
  id: 'inbound-a',
  jitterBufferDelay: 1.5,
  jitterBufferEmittedCount: 300,
  trackIdentifier: 'track-1',
};

const pairRecord: Record<string, unknown> = {
  type: 'candidate-pair',
  state: 'succeeded',
  nominated: true,
  currentRoundTripTime: 0.042,
};

describe('stats port', () => {
  test('reads the selected candidate pair and the active audio inbound sample', async () => {
    const { port } = makePort(report([['pair', pairRecord], ['inbound', inboundRecord]]));
    await port.refresh();

    expect(port.selectedCandidateRtt()).toEqual({ currentRoundTripTime: 0.042 });
    expect(port.inbound()).toMatchObject({
      sessionEpoch: 'session-1',
      statsId: 'inbound-a',
      stream: 'track-1',
      epoch: 'audio-1',
      emitted: 300,
      delay: 1.5,
      sampledAtMs: 1234,
    });
  });

  test('missing candidate pair and inbound stats return undefined, never 0', async () => {
    const { port } = makePort(report([['pair', { type: 'candidate-pair', state: 'in-progress' }]]));
    await port.refresh();

    expect(port.selectedCandidateRtt()).toBeUndefined();
    expect(port.selectedCandidateRtt()).not.toEqual({ currentRoundTripTime: 0 });
    expect(port.inbound()).toBeUndefined();
  });

  test('an inbound report without counters is not fabricated into a zero sample', async () => {
    const { port } = makePort(
      report([['inbound', { type: 'inbound-rtp', kind: 'audio', id: 'inbound-a' }]]),
    );
    await port.refresh();
    expect(port.inbound()).toBeUndefined();
  });

  test('no peer connection leaves both reads undefined', async () => {
    const { port } = makePort(report([]), { pc: false });
    await port.refresh();
    expect(port.selectedCandidateRtt()).toBeUndefined();
    expect(port.inbound()).toBeUndefined();
  });

  test('refresh never overlaps a getStats call already in flight', async () => {
    const pending: { resolve: ((value: RTCStatsReport) => void) | null } = { resolve: null };
    let calls = 0;
    const port = createStatsPort({
      peerConnection: () => ({
        getStats: () => {
          calls += 1;
          return new Promise<RTCStatsReport>(resolve => {
            pending.resolve = resolve;
          });
        },
      }),
      sessionEpoch: () => 'session-1',
      audioEpoch: () => 'audio-1',
      clock: () => 1,
    });

    const first = port.refresh();
    const second = port.refresh();
    expect(calls).toBe(1);

    pending.resolve?.(report([['pair', pairRecord]]));
    await Promise.all([first, second]);
    expect(calls).toBe(1);
  });
});
