import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { MixAck, MixPatch } from '../../protocol';
import { createMusicianSignaling } from '../signaling';

/** A minimal `WebSocket` double: captures frames and lets the test drive lifecycle events. */
class FakeWebSocket {
  static instances: FakeWebSocket[] = [];

  readonly url: string;
  readonly sent: string[] = [];
  onopen: ((event: unknown) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.onclose?.({});
  }

  open(): void {
    this.onopen?.({});
  }

  message(payload: unknown): void {
    this.onmessage?.({ data: JSON.stringify(payload) });
  }
}

const flush = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));

function locationWith(hash: string): Location {
  const href = `https://host.local:8443/join${hash}`;
  return {
    href,
    hash,
    protocol: 'https:',
    host: 'host.local:8443',
    pathname: '/join',
    search: '',
  } as unknown as Location;
}

function errorEnvelope(requestId: string, hostEpoch: string, code = 'STALE_EPOCH') {
  return {
    v: 1,
    type: 'error',
    requestId,
    hostEpoch,
    sessionEpoch: null,
    payload: { code, message: 'stale epoch', retryable: true },
  };
}

const ack: MixAck = {
  acceptedRevision: '12' as MixAck['acceptedRevision'],
  catalogRevision: '1' as MixAck['catalogRevision'],
  canonicalSettings: {
    catalogRevision: '1' as MixAck['canonicalSettings']['catalogRevision'],
    mixRevision: '12' as MixAck['canonicalSettings']['mixRevision'],
    sources: [{ sourceId: 'ch1', gainDb: -9, muted: false }],
    masterDb: 0,
    masterMuted: false,
  },
};

const patch: MixPatch = {
  baseRevision: '1' as MixPatch['baseRevision'],
  catalogRevision: '1' as MixPatch['catalogRevision'],
  sources: [{ sourceId: 'ch1', gainDb: -9, muted: false }],
  masterDb: 0,
  masterMuted: false,
};

/** Drive a connection to readiness, adopting the host epoch from the probe reply. */
async function connectReady(signalingReturn: { connect(): Promise<void> }) {
  const pending = signalingReturn.connect();
  await flush();
  const ws = FakeWebSocket.instances.at(-1)!;
  ws.open();
  await flush();
  const probe = JSON.parse(ws.sent[0]) as { requestId: string };
  ws.message(errorEnvelope(probe.requestId, 'host-epoch-1'));
  await pending;
  return ws;
}

beforeEach(() => {
  FakeWebSocket.instances = [];
});

afterEach(() => {
  vi.useRealTimers();
});

describe('musician signaling pairing', () => {
  test('posts the token once with credentials and clears it from the URL', async () => {
    const fetchImpl = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({ sessionEpoch: 'session-1' }),
    }));
    const history = { replaceState: vi.fn() } as unknown as History;
    const signaling = createMusicianSignaling({
      fetchImpl: fetchImpl as never,
      webSocket: FakeWebSocket as unknown as typeof WebSocket,
      location: locationWith('#t=one-time-token'),
      history,
      epochProbeTimeoutMs: 5,
    });

    const ws = await connectReady(signaling);

    expect(fetchImpl).toHaveBeenCalledTimes(1);
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/v1/pair');
    expect(init.method).toBe('POST');
    expect(init.credentials).toBe('include');
    expect(JSON.parse(init.body as string)).toEqual({ token: 'one-time-token' });
    expect(history.replaceState).toHaveBeenCalledWith(null, '', '/join');
    expect(ws.url).toBe('wss://host.local:8443/api/v1/ws');

    // A second connect is idempotent and never re-posts the single-use token.
    await signaling.connect();
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  test('a token already cleared is not posted again on a fresh client with no fragment', async () => {
    const fetchImpl = vi.fn();
    const signaling = createMusicianSignaling({
      fetchImpl: fetchImpl as never,
      webSocket: FakeWebSocket as unknown as typeof WebSocket,
      location: locationWith(''),
      history: { replaceState: vi.fn() } as unknown as History,
      epochProbeTimeoutMs: 5,
    });
    await connectReady(signaling);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

describe('musician signaling request correlation', () => {
  test('requestMix resolves on the matching mix.ack', async () => {
    const signaling = createMusicianSignaling({
      fetchImpl: vi.fn() as never,
      webSocket: FakeWebSocket as unknown as typeof WebSocket,
      location: locationWith(''),
      history: { replaceState: vi.fn() } as unknown as History,
      epochProbeTimeoutMs: 5,
    });
    const ws = await connectReady(signaling);

    const pending = signaling.requestMix(patch);
    await flush();
    const frame = JSON.parse(ws.sent.at(-1)!);
    expect(frame.type).toBe('mix.patch');
    expect(frame.hostEpoch).toBe('host-epoch-1');

    ws.message({
      v: 1,
      type: 'mix.ack',
      requestId: frame.requestId,
      hostEpoch: 'host-epoch-1',
      sessionEpoch: 'session-1',
      payload: ack,
    });

    await expect(pending).resolves.toMatchObject({ acceptedRevision: '12' });
  });

  test('requestMix rejects with a protocol error for its requestId', async () => {
    const signaling = createMusicianSignaling({
      fetchImpl: vi.fn() as never,
      webSocket: FakeWebSocket as unknown as typeof WebSocket,
      location: locationWith(''),
      history: { replaceState: vi.fn() } as unknown as History,
      epochProbeTimeoutMs: 5,
    });
    const ws = await connectReady(signaling);

    const pending = signaling.requestMix(patch);
    await flush();
    const frame = JSON.parse(ws.sent.at(-1)!);
    ws.message({
      v: 1,
      type: 'error',
      requestId: frame.requestId,
      hostEpoch: 'host-epoch-1',
      sessionEpoch: 'session-1',
      payload: { code: 'REVISION_CONFLICT', message: 'conflict', retryable: true },
    });

    await expect(pending).rejects.toMatchObject({ code: 'REVISION_CONFLICT', retryable: true });
  });

  test('requestMix times out rather than hanging forever', async () => {
    const signaling = createMusicianSignaling({
      fetchImpl: vi.fn() as never,
      webSocket: FakeWebSocket as unknown as typeof WebSocket,
      location: locationWith(''),
      history: { replaceState: vi.fn() } as unknown as History,
      epochProbeTimeoutMs: 5,
      requestTimeoutMs: 10,
    });
    await connectReady(signaling);

    await expect(signaling.requestMix(patch)).rejects.toThrow(/timed out/i);
  });
});
